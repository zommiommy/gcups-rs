use core::fmt;
use core::time::Duration;

use rusb::{Context, Device, DeviceHandle, UsbContext};

use crate::device::{DeviceInfo, DeviceSelector, UpsTransport, supported_transport};
use crate::error::Error;
use crate::parse::{parse_current, parse_nominal};
use crate::shutdown::{DescriptorShutdownDelay, MegatecShutdownDelay, ShutdownDelay};
use crate::status::{NominalParams, UpsStatus};
use crate::wire::{
    ACK_RESPONSE, B_REQUEST, BM_REQUEST_TYPE, BUF_SIZE, CYPRESS_INTERRUPT_IN,
    CYPRESS_OUTPUT_REPORT, CYPRESS_PACKET_SIZE, CYPRESS_SET_REPORT,
    CYPRESS_SET_REPORT_REQUEST_TYPE, DESC_TYPE_STRING, MEGATEC_MAX_COMMAND_LEN, W_INDEX,
    cypress_report, decode_ascii_response, decode_string_descriptor, report,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(3000);

/// How long to wait for an optional acknowledgement after a fire-and-forget
/// Cypress command. Bounded well below `DEFAULT_TIMEOUT` so commands the device
/// never answers (the common case) return promptly instead of blocking.
const CYPRESS_ACK_TIMEOUT: Duration = Duration::from_millis(250);

/// Handle to an open GreenCell UPS device.
///
/// Created via [`Ups::open`]. All methods perform synchronous USB I/O.
pub struct Ups {
    vid: u16,
    pid: u16,
    transport: UpsTransport,
    handle: DeviceHandle<Context>,
    timeout: Duration,
}

fn enumerate(ctx: &Context) -> Result<Vec<(DeviceInfo, Device<Context>)>, Error> {
    let devices = ctx.devices()?;
    let mut supported = Vec::new();

    for device in devices.iter() {
        let descriptor = device.device_descriptor()?;
        let vid = descriptor.vendor_id();
        let pid = descriptor.product_id();
        let Some(transport) = supported_transport(vid, pid) else {
            continue;
        };

        let info = DeviceInfo {
            vid,
            pid,
            bus: device.bus_number(),
            address: device.address(),
            transport,
        };
        supported.push((info, device));
    }

    supported.sort_by_key(|(info, _)| {
        (
            match info.transport {
                UpsTransport::Descriptor => 0,
                UpsTransport::CypressHid => 1,
            },
            info.bus,
            info.address,
        )
    });

    Ok(supported)
}

fn list_supported_devices(ctx: &Context) -> Result<Vec<DeviceInfo>, Error> {
    Ok(enumerate(ctx)?.into_iter().map(|(info, _)| info).collect())
}

fn select_device(devices: &[DeviceInfo], selector: Option<DeviceSelector>) -> Result<usize, Error> {
    let Some(selector) = selector else {
        return match devices {
            [] => Err(Error::DeviceNotFound),
            [_] => Ok(0),
            _ => Err(Error::AmbiguousDeviceAuto {
                count: devices.len(),
            }),
        };
    };

    let mut count = 0;
    let mut selected = None;
    for (i, device) in devices.iter().enumerate() {
        if selector.matches(*device) {
            count += 1;
            selected = Some(i);
        }
    }

    match (count, selected) {
        (0, _) => Err(Error::DeviceNotFoundBySelector { selector }),
        (1, Some(i)) => Ok(i),
        _ => Err(Error::AmbiguousDeviceSelector { selector, count }),
    }
}

fn open_selected(info: DeviceInfo, device: Device<Context>) -> Result<Ups, Error> {
    let handle = device.open()?;
    let _ = handle.set_auto_detach_kernel_driver(true);
    // Some backends need an explicit claim; ignore failure.
    let _ = handle.claim_interface(0);

    Ok(Ups {
        vid: info.vid,
        pid: info.pid,
        transport: info.transport,
        handle,
        timeout: DEFAULT_TIMEOUT,
    })
}

impl Ups {
    /// Open a supported UPS.
    ///
    /// Auto-detection succeeds only when exactly one supported UPS is attached.
    /// If multiple UPSes are connected, use [`Ups::list_devices`] and then
    /// [`Ups::open_with_selector`].
    pub fn open() -> Result<Self, Error> {
        Self::open_inner(None)
    }

    /// Open a supported UPS selected by VID:PID, optionally with USB bus/address.
    pub fn open_with_selector(selector: DeviceSelector) -> Result<Self, Error> {
        Self::open_inner(Some(selector))
    }

    fn open_inner(selector: Option<DeviceSelector>) -> Result<Self, Error> {
        let ctx = Context::new()?;
        let mut devices = enumerate(&ctx)?;
        let infos: Vec<DeviceInfo> = devices.iter().map(|(info, _)| *info).collect();
        let index = select_device(&infos, selector)?;
        let (info, device) = devices.swap_remove(index);
        open_selected(info, device)
    }

    /// List supported UPS devices currently visible on the USB bus.
    pub fn list_devices() -> Result<Vec<DeviceInfo>, Error> {
        let ctx = Context::new()?;
        list_supported_devices(&ctx)
    }

    /// Override the USB control transfer timeout (default: 3 000 ms).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Read the protocol identifier string.
    pub fn protocol(&self) -> Result<String, Error> {
        self.read_descriptor(report::PROTOCOL)
    }

    /// Read the protocol version string.
    pub fn protocol_version(&self) -> Result<String, Error> {
        self.read_descriptor(report::PROTOCOL_VERSION)
    }

    /// Read the device info string (e.g. `"2000VA"`).
    pub fn device_info(&self) -> Result<String, Error> {
        let raw = self.read_descriptor(report::INFO)?;
        Ok(raw.trim().trim_start_matches('#').trim().to_owned())
    }

    /// Read the nominal (rated) parameters.
    pub fn nominal_params(&self) -> Result<NominalParams, Error> {
        let raw = self.read_descriptor(report::NOMINAL_PARAMS)?;
        parse_nominal(&raw)
    }

    /// Read the full live status (combines nominal + current reports).
    ///
    /// Performs two USB transactions: one for the rated specs and one for
    /// the live readings. When polling in a loop, prefer
    /// [`current_status`](Self::current_status) with a cached
    /// [`NominalParams`] to avoid re-reading the rated specs each tick.
    pub fn status(&self) -> Result<UpsStatus, Error> {
        let nominal = self.nominal_params()?;
        self.current_status(&nominal)
    }

    /// Read live current parameters, parsed against a known nominal.
    ///
    /// Nominal parameters are the UPS's rated specs — they don't change
    /// at runtime, so a monitoring loop should fetch them once and reuse
    /// the reference. This performs exactly one USB transaction per call.
    pub fn current_status(&self, nominal: &NominalParams) -> Result<UpsStatus, Error> {
        let raw = self.read_descriptor(report::CURRENT_PARAMS)?;
        parse_current(&raw, nominal.clone())
    }

    /// Start a short (~10 s) battery self-test.
    pub fn short_test(&self) -> Result<(), Error> {
        self.send_command(report::SHORT_TEST)
    }

    /// Start a long (~10 min) battery self-test.
    pub fn long_test(&self) -> Result<(), Error> {
        self.send_command(report::LONG_TEST)
    }

    /// Cancel a running battery self-test.
    pub fn cancel_test(&self) -> Result<(), Error> {
        self.send_command(report::CANCEL_TEST)
    }

    /// Toggle the UPS beeper on/off.
    pub fn toggle_beeper(&self) -> Result<(), Error> {
        self.send_command(report::BEEPER_TOGGLE)
    }

    /// Schedule a UPS shutdown after `delay`.
    ///
    /// The actual delay is quantized to the nearest supported step
    /// (see [`ShutdownDelay`]). The UPS powers off and stays off.
    ///
    /// On the Cypress/Megatec transport "stay off" is encoded as `S<n>R0000`
    /// (the NUT stay-off convention); some firmware ignores it and restarts
    /// when mains returns.
    pub fn shutdown(&self, delay: Duration) -> Result<ShutdownDelay, Error> {
        match self.transport {
            UpsTransport::Descriptor => {
                let sd = DescriptorShutdownDelay::from_duration(delay);
                self.send_descriptor_command(sd.shutdown_report)?;
                Ok(sd.delay)
            }
            UpsTransport::CypressHid => {
                let sd = MegatecShutdownDelay::from_duration(delay);
                let mut command = [0; MEGATEC_MAX_COMMAND_LEN];
                let len = sd.write_shutdown_command(&mut command, true);
                self.send_cypress_command(report::SHUTDOWN, &command[..len])?;
                Ok(sd.delay)
            }
        }
    }

    /// Schedule a UPS shutdown after `delay`, with automatic power restore.
    ///
    /// The UPS powers off, then restores power once mains returns.
    pub fn shutdown_and_restore(&self, delay: Duration) -> Result<ShutdownDelay, Error> {
        match self.transport {
            UpsTransport::Descriptor => {
                let sd = DescriptorShutdownDelay::from_duration(delay);
                self.send_descriptor_command(sd.restore_report)?;
                Ok(sd.delay)
            }
            UpsTransport::CypressHid => {
                let sd = MegatecShutdownDelay::from_duration(delay);
                let mut command = [0; MEGATEC_MAX_COMMAND_LEN];
                let len = sd.write_shutdown_command(&mut command, false);
                self.send_cypress_command(report::SHUTDOWN_RESTORE, &command[..len])?;
                Ok(sd.delay)
            }
        }
    }

    /// Cancel a pending shutdown.
    pub fn cancel_shutdown(&self) -> Result<(), Error> {
        self.send_command(report::CANCEL_SHUTDOWN)
    }

    /// Cancel a pending shutdown-and-restore sequence.
    pub fn cancel_shutdown_restore(&self) -> Result<(), Error> {
        self.send_command(report::CANCEL_SHUTDOWN_RESTORE)
    }

    /// Cancel a pending shutdown-return sequence.
    pub fn cancel_shutdown_return(&self) -> Result<(), Error> {
        self.send_command(report::CANCEL_SHUTDOWN_RETURN)
    }

    /// Wake up / restore power (same wire command as cancel-shutdown).
    pub fn wake_up(&self) -> Result<(), Error> {
        self.cancel_shutdown()
    }

    /// Read a raw report by descriptor index / logical report ID, decoded to ASCII.
    pub fn read_descriptor(&self, index: u8) -> Result<String, Error> {
        match self.transport {
            UpsTransport::Descriptor => self.read_string_descriptor(index),
            UpsTransport::CypressHid => {
                let report = cypress_report(index)?;
                if report.expects_reply {
                    self.cypress_query(index, report.command)
                } else {
                    self.cypress_write_command(index, report.command)?;
                    Ok(self
                        .cypress_read_optional_response(index)?
                        .unwrap_or_default())
                }
            }
        }
    }

    /// Send a command.
    fn send_command(&self, report_id: u8) -> Result<(), Error> {
        match self.transport {
            UpsTransport::Descriptor => self.send_descriptor_command(report_id),
            UpsTransport::CypressHid => {
                let report = cypress_report(report_id)?;
                self.send_cypress_command(report_id, report.command)
            }
        }
    }

    fn read_string_descriptor(&self, index: u8) -> Result<String, Error> {
        let mut buf = [0u8; BUF_SIZE];
        let n = self.handle.read_control(
            BM_REQUEST_TYPE,
            B_REQUEST,
            DESC_TYPE_STRING | u16::from(index),
            W_INDEX,
            &mut buf,
            self.timeout,
        )?;

        if n < 2 {
            return Err(Error::ResponseTooShort {
                report_id: index,
                len: n,
            });
        }

        Ok(decode_string_descriptor(&buf[..n]))
    }

    fn send_descriptor_command(&self, report_id: u8) -> Result<(), Error> {
        let resp = self.read_string_descriptor(report_id)?;
        if resp.trim() == ACK_RESPONSE {
            Ok(())
        } else {
            Err(Error::NotAcknowledged { report_id })
        }
    }

    fn cypress_query(&self, report_id: u8, command: &[u8]) -> Result<String, Error> {
        self.cypress_write_command(report_id, command)?;
        self.cypress_read_response(report_id, self.timeout)
    }

    fn cypress_write_command(&self, report_id: u8, command: &[u8]) -> Result<(), Error> {
        for chunk in command.chunks(CYPRESS_PACKET_SIZE) {
            let mut packet = [0; CYPRESS_PACKET_SIZE];
            packet[..chunk.len()].copy_from_slice(chunk);

            let n = self.handle.write_control(
                CYPRESS_SET_REPORT_REQUEST_TYPE,
                CYPRESS_SET_REPORT,
                CYPRESS_OUTPUT_REPORT,
                W_INDEX,
                &packet,
                self.timeout,
            )?;

            if n != CYPRESS_PACKET_SIZE {
                return Err(Error::ShortWrite {
                    report_id,
                    len: n,
                    expected: CYPRESS_PACKET_SIZE,
                });
            }
        }

        Ok(())
    }

    fn cypress_read_response(&self, report_id: u8, timeout: Duration) -> Result<String, Error> {
        let mut buf = [0; BUF_SIZE];
        let mut len = 0;

        while len <= BUF_SIZE - CYPRESS_PACKET_SIZE {
            let n = self.handle.read_interrupt(
                CYPRESS_INTERRUPT_IN,
                &mut buf[len..len + CYPRESS_PACKET_SIZE],
                timeout,
            )?;

            if n == 0 {
                return Err(Error::ResponseTooShort { report_id, len });
            }

            len += n;
            if buf[..len].contains(&b'\r') {
                return Ok(decode_ascii_response(&buf[..len]));
            }
        }

        Ok(decode_ascii_response(&buf[..len]))
    }

    fn cypress_read_optional_response(&self, report_id: u8) -> Result<Option<String>, Error> {
        match self.cypress_read_response(report_id, CYPRESS_ACK_TIMEOUT) {
            Ok(resp) => Ok(Some(resp)),
            Err(Error::Usb(rusb::Error::Timeout)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn send_cypress_command(&self, report_id: u8, command: &[u8]) -> Result<(), Error> {
        self.cypress_write_command(report_id, command)?;

        let Some(resp) = self.cypress_read_optional_response(report_id)? else {
            return Ok(());
        };

        let resp = resp.trim();
        if resp.is_empty() || resp.starts_with("ACK") || resp.starts_with("(ACK") {
            Ok(())
        } else {
            Err(Error::NotAcknowledged { report_id })
        }
    }
}

impl fmt::Debug for Ups {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ups")
            .field("vid", &format_args!("{:04x}", self.vid))
            .field("pid", &format_args!("{:04x}", self.pid))
            .field("transport", &self.transport)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{CYPRESS_PID, CYPRESS_VID};

    #[test]
    fn select_device_rejects_ambiguous_auto_and_vid_pid() {
        let devices = [
            DeviceInfo {
                vid: CYPRESS_VID,
                pid: CYPRESS_PID,
                bus: 1,
                address: 4,
                transport: UpsTransport::CypressHid,
            },
            DeviceInfo {
                vid: CYPRESS_VID,
                pid: CYPRESS_PID,
                bus: 1,
                address: 5,
                transport: UpsTransport::CypressHid,
            },
        ];

        assert!(matches!(
            select_device(&devices, None),
            Err(Error::AmbiguousDeviceAuto { count: 2 })
        ));

        assert!(matches!(
            select_device(
                &devices,
                Some(DeviceSelector::new(CYPRESS_VID, CYPRESS_PID))
            ),
            Err(Error::AmbiguousDeviceSelector { count: 2, .. })
        ));

        let index = select_device(
            &devices,
            Some(DeviceSelector::with_location(
                CYPRESS_VID,
                CYPRESS_PID,
                1,
                5,
            )),
        )
        .unwrap();
        assert_eq!(devices[index].address, 5);
    }
}
