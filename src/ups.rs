use core::cell::{Cell, RefCell};
use core::fmt;
use core::time::Duration;
use std::io::{Read, Write};

use rusb::{Context, Device, DeviceHandle, UsbContext};
use serialport::{
    ClearBuffer, DataBits, Parity, SerialPort, SerialPortType, StopBits, available_ports,
};

use crate::device::{DeviceInfo, DeviceSelector, UpsTransport, supported_transport};
use crate::error::Error;
use crate::parse::{parse_current, parse_cypress_t_current, parse_nominal};
use crate::shutdown::{
    DescriptorShutdownDelay, MegatecShutdownDelay, ProlificShutdownDelay, ShutdownDelay,
};
use crate::status::{NominalParams, UpsStatus};
use crate::wire::{
    ACK_RESPONSE, B_REQUEST, BM_REQUEST_TYPE, BUF_SIZE, CYPRESS_FEATURE_REPORT,
    CYPRESS_INTERRUPT_IN, CYPRESS_OUTPUT_REPORT, CYPRESS_PACKET_SIZE, CYPRESS_SET_REPORT,
    CYPRESS_SET_REPORT_REQUEST_TYPE, DESC_TYPE_STRING, MEGATEC_MAX_COMMAND_LEN, W_INDEX,
    cypress_report, decode_ascii_response, decode_string_descriptor, descriptor_payload, report,
    response_payload,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(3000);

/// How long to wait for an optional acknowledgement after a fire-and-forget
/// Cypress command. Bounded well below `DEFAULT_TIMEOUT` so commands the device
/// never answers (the common case) return promptly instead of blocking.
const CYPRESS_ACK_TIMEOUT: Duration = Duration::from_millis(250);
/// Per-read timeout while draining the interrupt-IN endpoint at open. Buffered
/// packets return within a poll interval, so an empty endpoint costs a single
/// such wait — paid once per open, never on the polling hot path.
const CYPRESS_DRAIN_TIMEOUT: Duration = Duration::from_millis(50);
const SERIAL_READ_TIMEOUT: Duration = Duration::from_millis(100);

/// How long to wait for an optional acknowledgement after a fire-and-forget
/// Prolific/Megatec order command. Bounds the no-reply wait so order commands
/// (which the device never answers) return promptly instead of blocking for the
/// full query timeout. Matches the official app's 1000 ms queue timeout.
const SERIAL_COMMAND_TIMEOUT: Duration = Duration::from_millis(1000);

/// Hex/ASCII dump of a transfer to stderr, gated on the `GCUPS_DEBUG`
/// environment variable. Lets us diagnose transports on hardware we cannot
/// test directly.
fn trace(dir: &str, report_id: u8, data: &[u8]) {
    use core::fmt::Write as _;
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("GCUPS_DEBUG").is_some()) {
        return;
    }
    let mut hex = String::new();
    for &b in data {
        let _ = write!(hex, "{b:02x} ");
    }
    let ascii: String = data
        .iter()
        .map(|&b| {
            if (0x20..0x7f).contains(&b) {
                char::from(b)
            } else {
                '.'
            }
        })
        .collect();
    eprintln!(
        "[gcups] {dir} 0x{report_id:02x} ({} bytes): {hex}|{ascii}|",
        data.len()
    );
}

/// Handle to an open GreenCell UPS device.
///
/// Created via [`Ups::open`]. All methods perform synchronous transport I/O.
pub struct Ups {
    vid: u16,
    pid: u16,
    transport: UpsTransport,
    handle: RefCell<UpsHandle>,
    timeout: Duration,
    /// Cached Cypress QS sub-protocol (`V` or `T`), learned from the first `QS`
    /// reply or a `M` query. Drives command-acknowledgement behaviour, which
    /// differs between the two. Always `None` for non-Cypress transports.
    cypress_protocol: Cell<Option<CypressProtocol>>,
}

enum UpsHandle {
    Usb(DeviceHandle<Context>),
    Serial(Box<dyn SerialPort>),
}

/// GreenCell Cypress QS sub-protocol variant. `V` replies in ASCII and
/// acknowledges commands with `ACK`; `T` replies in a packed binary frame and
/// does not acknowledge commands at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CypressProtocol {
    V,
    T,
}

struct DeviceCandidate {
    info: DeviceInfo,
    usb_device: Option<Device<Context>>,
}

fn enumerate_usb(ctx: &Context) -> Result<Vec<DeviceCandidate>, Error> {
    let devices = ctx.devices()?;
    let mut supported = Vec::new();

    for device in devices.iter() {
        let descriptor = device.device_descriptor()?;
        let vid = descriptor.vendor_id();
        let pid = descriptor.product_id();
        let Some(transport) = supported_transport(vid, pid) else {
            continue;
        };
        if transport == UpsTransport::ProlificSerial {
            continue;
        }

        supported.push(DeviceCandidate {
            info: DeviceInfo {
                vid,
                pid,
                bus: device.bus_number(),
                address: device.address(),
                transport,
                serial_path: None,
            },
            usb_device: Some(device),
        });
    }

    Ok(supported)
}

fn enumerate_serial() -> Result<Vec<DeviceCandidate>, Error> {
    let mut supported = Vec::new();

    for port in available_ports().map_err(|e| Error::Serial {
        detail: format!("serial port enumeration failed: {e}"),
    })? {
        let SerialPortType::UsbPort(info) = port.port_type else {
            continue;
        };
        let Some(transport) = supported_transport(info.vid, info.pid) else {
            continue;
        };
        if transport != UpsTransport::ProlificSerial {
            continue;
        }

        supported.push(DeviceCandidate {
            info: DeviceInfo {
                vid: info.vid,
                pid: info.pid,
                bus: 0,
                address: 0,
                transport,
                serial_path: Some(port.port_name.clone()),
            },
            usb_device: None,
        });
    }

    Ok(supported)
}

fn enumerate(ctx: &Context) -> Result<Vec<DeviceCandidate>, Error> {
    let mut supported = enumerate_usb(ctx)?;
    supported.extend(enumerate_serial()?);
    supported.sort_by(|a, b| {
        let order = |transport| match transport {
            UpsTransport::Descriptor => 0,
            UpsTransport::CypressHid => 1,
            UpsTransport::ProlificSerial => 2,
        };
        order(a.info.transport)
            .cmp(&order(b.info.transport))
            .then(a.info.bus.cmp(&b.info.bus))
            .then(a.info.address.cmp(&b.info.address))
            .then(a.info.serial_path.cmp(&b.info.serial_path))
    });
    Ok(supported)
}

fn list_supported_devices(ctx: &Context) -> Result<Vec<DeviceInfo>, Error> {
    Ok(enumerate(ctx)?
        .into_iter()
        .map(|candidate| candidate.info)
        .collect())
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
        if selector.matches(device) {
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

fn open_selected(candidate: DeviceCandidate) -> Result<Ups, Error> {
    let info = candidate.info;
    let handle = match info.transport {
        UpsTransport::Descriptor | UpsTransport::CypressHid => {
            let device = candidate.usb_device.expect("usb candidate missing device");
            let handle = device.open()?;
            let _ = handle.set_auto_detach_kernel_driver(true);
            let _ = handle.claim_interface(0);
            UpsHandle::Usb(handle)
        }
        UpsTransport::ProlificSerial => {
            let path = info
                .serial_path
                .as_ref()
                .expect("serial candidate missing path");
            let port = serialport::new(path, 2400)
                .data_bits(DataBits::Eight)
                .stop_bits(StopBits::One)
                .parity(Parity::None)
                .timeout(SERIAL_READ_TIMEOUT)
                .open()
                .map_err(|e| Error::Serial {
                    detail: format!("open serial port {path:?}: {e}"),
                })?;
            UpsHandle::Serial(port)
        }
    };

    let ups = Ups {
        vid: info.vid,
        pid: info.pid,
        transport: info.transport,
        handle: RefCell::new(handle),
        timeout: DEFAULT_TIMEOUT,
        cypress_protocol: Cell::new(None),
    };

    // GreenCell Cypress firmware buffers a command's reply in the interrupt
    // endpoint and waits for the host to read it. A previous run killed
    // mid-reply (e.g. Ctrl-C during `watch`) leaves a stale tail there; flush
    // it before the first command so replies stay aligned with their queries.
    if matches!(ups.transport, UpsTransport::CypressHid) {
        ups.cypress_drain();
    }

    Ok(ups)
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

    /// Open a supported UPS selected by VID:PID, optionally with USB bus/address
    /// or a serial-port path.
    pub fn open_with_selector(selector: DeviceSelector) -> Result<Self, Error> {
        Self::open_inner(Some(selector))
    }

    fn open_inner(selector: Option<DeviceSelector>) -> Result<Self, Error> {
        let ctx = Context::new()?;
        let mut devices = enumerate(&ctx)?;
        let infos: Vec<DeviceInfo> = devices
            .iter()
            .map(|candidate| candidate.info.clone())
            .collect();
        let index = select_device(&infos, selector)?;
        let candidate = devices.swap_remove(index);
        open_selected(candidate)
    }

    /// List supported UPS devices currently visible on the bus.
    pub fn list_devices() -> Result<Vec<DeviceInfo>, Error> {
        let ctx = Context::new()?;
        list_supported_devices(&ctx)
    }

    /// Override the transport timeout (default: 3 000 ms).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
        if let UpsHandle::Serial(port) = &mut *self.handle.borrow_mut() {
            let _ = port.set_timeout(timeout.min(SERIAL_READ_TIMEOUT));
        }
    }

    /// Read the protocol identifier string.
    pub fn protocol(&self) -> Result<String, Error> {
        match self.transport {
            UpsTransport::CypressHid => Ok("QS".to_owned()),
            _ => {
                let raw = self.read_descriptor(report::PROTOCOL)?;
                Ok(Self::clean_report_text(&raw))
            }
        }
    }

    /// Read the protocol version string.
    pub fn protocol_version(&self) -> Result<String, Error> {
        match self.transport {
            UpsTransport::CypressHid => {
                Ok(Self::cypress_protocol_marker(self.cypress_protocol()?).to_owned())
            }
            _ => {
                let raw = self.read_descriptor(report::PROTOCOL_VERSION)?;
                Ok(Self::clean_report_text(&raw))
            }
        }
    }

    /// Read the device info string (e.g. `"2000VA"`).
    pub fn device_info(&self) -> Result<String, Error> {
        if matches!(self.transport, UpsTransport::CypressHid) {
            return Err(Error::UnsupportedReport {
                report_id: report::INFO,
            });
        }

        let raw = self.read_descriptor(report::INFO)?;
        Ok(Self::clean_report_text(&raw)
            .trim_start_matches('#')
            .trim()
            .to_owned())
    }

    /// Read the nominal (rated) parameters.
    pub fn nominal_params(&self) -> Result<NominalParams, Error> {
        match self.transport {
            UpsTransport::Descriptor => {
                let raw = self.read_descriptor(report::NOMINAL_PARAMS)?;
                parse_nominal(&raw)
            }
            UpsTransport::CypressHid => self.cypress_nominal_params(),
            UpsTransport::ProlificSerial => {
                let raw = self.read_descriptor(report::NOMINAL_PARAMS)?;
                parse_nominal(&raw)
            }
        }
    }

    /// Read the full live status.
    ///
    /// Descriptor and Cypress `V` devices combine rated specs with current
    /// readings. Cypress `T` devices encode nominal and current values in the
    /// same `QS` frame. When polling in a loop, prefer
    /// [`current_status`](Self::current_status) with cached [`NominalParams`].
    pub fn status(&self) -> Result<UpsStatus, Error> {
        match self.transport {
            UpsTransport::Descriptor => {
                let nominal = self.nominal_params()?;
                self.current_status(&nominal)
            }
            UpsTransport::CypressHid => self.cypress_status(),
            UpsTransport::ProlificSerial => {
                let nominal = self.nominal_params()?;
                self.current_status(&nominal)
            }
        }
    }

    /// Read live current parameters, parsed against a known nominal.
    ///
    /// Nominal parameters are the UPS's rated specs — they don't change
    /// at runtime, so a monitoring loop should fetch them once and reuse
    /// the reference. This performs exactly one transport transaction per call.
    pub fn current_status(&self, nominal: &NominalParams) -> Result<UpsStatus, Error> {
        match self.transport {
            UpsTransport::Descriptor => {
                let raw = self.read_descriptor(report::CURRENT_PARAMS)?;
                parse_current(&raw, nominal.clone())
            }
            UpsTransport::CypressHid => self.cypress_current_status(Some(nominal)),
            UpsTransport::ProlificSerial => {
                let raw = self.read_descriptor(report::CURRENT_PARAMS)?;
                parse_current(&raw, nominal.clone())
            }
        }
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
                // The official app always sends the fixed `S.5R0000` (30 s, stay
                // off) and ignores the requested delay. We honour `delay` with
                // the standard Megatec `S<n>R0000` stay-off form instead.
                let sd = MegatecShutdownDelay::from_duration(delay);
                let mut command = [0; MEGATEC_MAX_COMMAND_LEN];
                let len = sd.write_shutdown_command(&mut command, true);
                self.send_cypress_command(report::SHUTDOWN, &command[..len])?;
                Ok(sd.delay)
            }
            UpsTransport::ProlificSerial => {
                let sd = ProlificShutdownDelay::from_duration(delay);
                self.serial_command(report::SHUTDOWN, sd.shutdown_command().as_bytes())?;
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
                // `S<n>` (no `R` suffix) shuts down, then auto-restores when
                // mains returns. The official app's QS restore path is broken
                // (`command$(undefined)`); honouring `delay` here is deliberate.
                let sd = MegatecShutdownDelay::from_duration(delay);
                let mut command = [0; MEGATEC_MAX_COMMAND_LEN];
                let len = sd.write_shutdown_command(&mut command, false);
                self.send_cypress_command(report::SHUTDOWN_RESTORE, &command[..len])?;
                Ok(sd.delay)
            }
            UpsTransport::ProlificSerial => {
                let sd = ProlificShutdownDelay::from_duration(delay);
                self.serial_command(
                    report::SHUTDOWN_RESTORE,
                    sd.shutdown_restore_command().as_bytes(),
                )?;
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
            UpsTransport::ProlificSerial => match index {
                report::PROTOCOL => Ok("Prolific".to_owned()),
                report::PROTOCOL_VERSION => Ok("prolific".to_owned()),
                _ => {
                    let command = Self::serial_query_command(index)?
                        .ok_or(Error::UnsupportedReport { report_id: index })?;
                    self.serial_query(index, command)
                }
            },
        }
    }

    /// Read a raw report by descriptor index / logical report ID and return the
    /// device's response bytes verbatim.
    ///
    /// Unlike [`read_descriptor`](Self::read_descriptor), the reply is *not*
    /// decoded to text: NUL bytes are kept and bytes `>= 0x80` are returned
    /// as-is rather than re-encoded to UTF-8. Binary replies — notably the
    /// Cypress `T` `QS` frame, which legitimately contains NULs and high bytes —
    /// are therefore dumped exactly as received instead of being corrupted. Use
    /// this to capture a faithful copy of a report for debugging or off-line
    /// decoding; [`read_descriptor`](Self::read_descriptor) remains the right
    /// choice for the ASCII reports the higher-level queries parse.
    pub fn read_report_raw(&self, index: u8) -> Result<Vec<u8>, Error> {
        match self.transport {
            UpsTransport::Descriptor => {
                let (buf, n) = self.read_descriptor_bytes(index)?;
                Ok(descriptor_payload(&buf[..n]))
            }
            UpsTransport::CypressHid => {
                let report = cypress_report(index)?;
                let raw = if report.expects_reply {
                    self.cypress_query_raw(index, report.command)?
                } else {
                    self.cypress_write_command(index, report.command)?;
                    self.cypress_read_optional_response_raw(index)?
                        .unwrap_or_default()
                };
                Ok(response_payload(&raw).to_vec())
            }
            UpsTransport::ProlificSerial => match index {
                report::PROTOCOL => Ok(b"Prolific".to_vec()),
                report::PROTOCOL_VERSION => Ok(b"prolific".to_vec()),
                _ => {
                    let command = Self::serial_query_command(index)?
                        .ok_or(Error::UnsupportedReport { report_id: index })?;
                    let out = self.serial_io(index, command, self.timeout)?;
                    if out.is_empty() {
                        return Err(Error::ResponseTooShort {
                            report_id: index,
                            len: 0,
                        });
                    }
                    Ok(response_payload(&out).to_vec())
                }
            },
        }
    }

    fn cypress_status(&self) -> Result<UpsStatus, Error> {
        self.cypress_current_status(None)
    }

    fn cypress_nominal_params(&self) -> Result<NominalParams, Error> {
        let raw = self.cypress_query_raw(report::CURRENT_PARAMS, b"QS\r")?;
        match self.classify_cypress_qs(&raw) {
            CypressProtocol::V => {
                let raw = self.cypress_query(report::NOMINAL_PARAMS, b"F\r")?;
                parse_nominal(&raw)
            }
            CypressProtocol::T => Ok(parse_cypress_t_current(&raw)?.nominal),
        }
    }

    fn cypress_current_status(&self, nominal: Option<&NominalParams>) -> Result<UpsStatus, Error> {
        let raw = self.cypress_query_raw(report::CURRENT_PARAMS, b"QS\r")?;
        match self.classify_cypress_qs(&raw) {
            CypressProtocol::V => {
                let nominal = match nominal {
                    Some(nominal) => nominal.clone(),
                    None => {
                        let raw = self.cypress_query(report::NOMINAL_PARAMS, b"F\r")?;
                        parse_nominal(&raw)?
                    }
                };
                parse_current(&decode_ascii_response(&raw), nominal)
            }
            CypressProtocol::T => parse_cypress_t_current(&raw),
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
            UpsTransport::ProlificSerial => self.send_serial_command(report_id),
        }
    }

    fn read_string_descriptor(&self, index: u8) -> Result<String, Error> {
        let (buf, n) = self.read_descriptor_bytes(index)?;
        Ok(decode_string_descriptor(&buf[..n]))
    }

    /// Issue the `GET_DESCRIPTOR(STRING, index)` control transfer and return the
    /// raw response buffer plus its length. Shared by the text decoder and the
    /// byte-faithful raw reader.
    fn read_descriptor_bytes(&self, index: u8) -> Result<([u8; BUF_SIZE], usize), Error> {
        let mut buf = [0u8; BUF_SIZE];
        let n = {
            let mut handle = self.handle.borrow_mut();
            let UpsHandle::Usb(handle) = &mut *handle else {
                return Err(Error::UnsupportedReport { report_id: index });
            };
            handle.read_control(
                BM_REQUEST_TYPE,
                B_REQUEST,
                DESC_TYPE_STRING | u16::from(index),
                W_INDEX,
                &mut buf,
                self.timeout,
            )?
        };
        trace("RX", index, &buf[..n]);

        if n < 2 {
            return Err(Error::ResponseTooShort {
                report_id: index,
                len: n,
            });
        }

        Ok((buf, n))
    }

    fn send_descriptor_command(&self, report_id: u8) -> Result<(), Error> {
        let resp = self.read_string_descriptor(report_id)?;
        if resp.trim() == ACK_RESPONSE {
            Ok(())
        } else {
            Err(Error::NotAcknowledged { report_id })
        }
    }

    /// Write `command` to the serial port and read a reply, bounded by `deadline`.
    ///
    /// Returns whatever bytes arrived (possibly empty). Callers decide whether an
    /// empty reply is an error (data queries) or success (fire-and-forget orders,
    /// which Megatec devices never answer).
    fn serial_io(
        &self,
        report_id: u8,
        command: &[u8],
        deadline: Duration,
    ) -> Result<Vec<u8>, Error> {
        trace("TX", report_id, command);
        let mut handle = self.handle.borrow_mut();
        let UpsHandle::Serial(port) = &mut *handle else {
            return Err(Error::UnsupportedReport { report_id });
        };
        let _ = port.clear(ClearBuffer::Input);
        port.write_all(command).map_err(|e| Error::Serial {
            detail: format!("write failed for report 0x{report_id:02x}: {e}"),
        })?;
        port.flush().map_err(|e| Error::Serial {
            detail: format!("flush failed for report 0x{report_id:02x}: {e}"),
        })?;

        let started = std::time::Instant::now();
        let mut out = Vec::new();
        let mut chunk = [0u8; 64];
        loop {
            match port.read(&mut chunk) {
                Ok(0) => {}
                Ok(n) => {
                    trace("RX", report_id, &chunk[..n]);
                    out.extend_from_slice(&chunk[..n]);
                    if out.contains(&b'\r') {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    if !out.is_empty() {
                        break;
                    }
                }
                Err(e) => {
                    return Err(Error::Serial {
                        detail: format!("read failed for report 0x{report_id:02x}: {e}"),
                    });
                }
            }
            if started.elapsed() >= deadline {
                break;
            }
        }

        Ok(out)
    }

    /// Issue a data query (`Q1`/`F`/`I`) and return the ASCII reply. A query that
    /// returns nothing within the timeout is an error — queries always reply.
    fn serial_query(&self, report_id: u8, command: &[u8]) -> Result<String, Error> {
        let out = self.serial_io(report_id, command, self.timeout)?;
        if out.is_empty() {
            return Err(Error::ResponseTooShort { report_id, len: 0 });
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Issue a fire-and-forget order (`T`/`TL`/`Q`/`C`/`CT`/`S…`). Megatec order
    /// commands return nothing on success; the official app synthesizes the
    /// `"UPS No Ack"` string after its queue timeout and treats it as OK. So an
    /// empty reply — or an explicit `ACK_RESPONSE` — is success; any other
    /// payload is a negative acknowledgement.
    fn serial_command(&self, report_id: u8, command: &[u8]) -> Result<(), Error> {
        let out = self.serial_io(report_id, command, SERIAL_COMMAND_TIMEOUT)?;
        let resp = String::from_utf8_lossy(&out);
        let resp = resp.trim();
        if resp.is_empty() || resp == ACK_RESPONSE {
            Ok(())
        } else {
            Err(Error::NotAcknowledged { report_id })
        }
    }

    fn serial_query_command(report_id: u8) -> Result<Option<&'static [u8]>, Error> {
        Ok(match report_id {
            report::CURRENT_PARAMS => Some(b"Q1\r"),
            report::INFO => Some(b"I\r"),
            report::NOMINAL_PARAMS => Some(b"F\r"),
            report::SHORT_TEST => Some(b"T\r"),
            report::LONG_TEST => Some(b"TL\r"),
            report::BEEPER_TOGGLE => Some(b"Q\r"),
            // The official Prolific set leaves CSR/CS empty (a no-op `\r`); we
            // map them to the real `C` cancel so the operations actually take
            // effect.
            report::CANCEL_SHUTDOWN
            | report::CANCEL_SHUTDOWN_RESTORE
            | report::CANCEL_SHUTDOWN_RETURN => Some(b"C\r"),
            report::CANCEL_TEST => Some(b"CT\r"),
            report::PROTOCOL | report::PROTOCOL_VERSION => None,
            _ => return Err(Error::UnsupportedReport { report_id }),
        })
    }

    fn send_serial_command(&self, report_id: u8) -> Result<(), Error> {
        let command =
            Self::serial_query_command(report_id)?.ok_or(Error::UnsupportedReport { report_id })?;
        self.serial_command(report_id, command)
    }

    fn cypress_query(&self, report_id: u8, command: &[u8]) -> Result<String, Error> {
        let raw = self.cypress_query_raw(report_id, command)?;
        Ok(decode_ascii_response(&raw))
    }

    fn cypress_query_raw(&self, report_id: u8, command: &[u8]) -> Result<Vec<u8>, Error> {
        self.cypress_write_command(report_id, command)?;
        self.cypress_read_response_raw(report_id, self.timeout)
    }

    fn cypress_write_command(&self, report_id: u8, command: &[u8]) -> Result<(), Error> {
        debug_assert!(
            command.len() <= MEGATEC_MAX_COMMAND_LEN,
            "cypress command exceeds packet buffer"
        );
        let len = command.len().max(CYPRESS_PACKET_SIZE);
        let mut packet = [0; MEGATEC_MAX_COMMAND_LEN];
        packet[..command.len()].copy_from_slice(command);
        let packet = &packet[..len];
        trace("TX", report_id, packet);

        // GreenCell firmware accepts the Megatec command as a HID OUTPUT report;
        // some units only accept it as a FEATURE report, so fall back like the
        // official app does.
        self.cypress_set_report(report_id, CYPRESS_OUTPUT_REPORT, packet)
            .or_else(|_| self.cypress_set_report(report_id, CYPRESS_FEATURE_REPORT, packet))
    }

    fn cypress_set_report(
        &self,
        report_id: u8,
        report_type: u16,
        packet: &[u8],
    ) -> Result<(), Error> {
        let n = {
            let mut handle = self.handle.borrow_mut();
            let UpsHandle::Usb(handle) = &mut *handle else {
                return Err(Error::UnsupportedReport { report_id });
            };
            handle.write_control(
                CYPRESS_SET_REPORT_REQUEST_TYPE,
                CYPRESS_SET_REPORT,
                report_type,
                W_INDEX,
                packet,
                self.timeout,
            )?
        };

        if n != packet.len() {
            return Err(Error::ShortWrite {
                report_id,
                len: n,
                expected: packet.len(),
            });
        }

        Ok(())
    }

    fn cypress_read_response(&self, report_id: u8, timeout: Duration) -> Result<String, Error> {
        let raw = self.cypress_read_response_raw(report_id, timeout)?;
        Ok(decode_ascii_response(&raw))
    }

    fn cypress_read_response_raw(
        &self,
        report_id: u8,
        timeout: Duration,
    ) -> Result<Vec<u8>, Error> {
        let mut buf = [0; BUF_SIZE];
        let mut len = 0;

        while len <= BUF_SIZE - CYPRESS_PACKET_SIZE {
            let n = {
                let mut handle = self.handle.borrow_mut();
                let UpsHandle::Usb(handle) = &mut *handle else {
                    return Err(Error::UnsupportedReport { report_id });
                };
                handle.read_interrupt(
                    CYPRESS_INTERRUPT_IN,
                    &mut buf[len..len + CYPRESS_PACKET_SIZE],
                    timeout,
                )?
            };
            trace("RX", report_id, &buf[len..len + n]);

            if n == 0 {
                return Err(Error::ResponseTooShort { report_id, len });
            }

            len += n;
            if buf[..len].contains(&b'\r') {
                return Ok(buf[..len].to_vec());
            }
        }

        Ok(buf[..len].to_vec())
    }

    /// Discard interrupt-IN data left buffered by a previous, possibly
    /// interrupted session before the first command.
    ///
    /// GreenCell Cypress firmware queues a command's reply in the interrupt
    /// endpoint and waits for the host to read it. A `watch` killed with Ctrl-C
    /// mid-reply leaves the unread tail in that FIFO; without this flush the
    /// next run reads the stale tail and every reply is shifted by one query —
    /// surfacing as e.g. `parse error for report 0x0d: missing '#' prefix`.
    /// This is the interrupt-endpoint analog of the input flush the serial
    /// transport performs before each write.
    fn cypress_drain(&self) {
        let mut handle = self.handle.borrow_mut();
        let UpsHandle::Usb(handle) = &mut *handle else {
            return;
        };
        let mut scratch = [0u8; CYPRESS_PACKET_SIZE];
        // A full reply is at most BUF_SIZE and only a tail can remain, so this
        // bound comfortably exceeds any leftover while capping a device that
        // streams without pause.
        for _ in 0..BUF_SIZE / CYPRESS_PACKET_SIZE {
            match handle.read_interrupt(CYPRESS_INTERRUPT_IN, &mut scratch, CYPRESS_DRAIN_TIMEOUT) {
                Ok(n) if n > 0 => trace("DRAIN", 0, &scratch[..n]),
                // Empty (timeout), zero-length packet, or a transient error: the
                // endpoint is clear or unreadable. Best-effort, so stop quietly.
                _ => return,
            }
        }
    }

    fn cypress_read_optional_response(&self, report_id: u8) -> Result<Option<String>, Error> {
        match self.cypress_read_response(report_id, CYPRESS_ACK_TIMEOUT) {
            Ok(resp) => Ok(Some(resp)),
            Err(Error::Usb(rusb::Error::Timeout)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Byte-faithful variant of [`Self::cypress_read_optional_response`]: returns
    /// the raw reply bytes (or `None` on the no-answer timeout) without the lossy
    /// text decode.
    fn cypress_read_optional_response_raw(&self, report_id: u8) -> Result<Option<Vec<u8>>, Error> {
        match self.cypress_read_response_raw(report_id, CYPRESS_ACK_TIMEOUT) {
            Ok(resp) => Ok(Some(resp)),
            Err(Error::Usb(rusb::Error::Timeout)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn send_cypress_command(&self, report_id: u8, command: &[u8]) -> Result<(), Error> {
        // Detect the sub-protocol *before* writing so the optional `M` query
        // cannot interleave with this command's reply.
        let protocol = self.cypress_protocol()?;
        self.cypress_write_command(report_id, command)?;
        // `V` acknowledges commands with `ACK`; `T` is fire-and-forget and never
        // replies, so reading would only stall and risk a false negative.
        let reply = match protocol {
            CypressProtocol::T => None,
            CypressProtocol::V => self.cypress_read_optional_response(report_id)?,
        };
        Self::cypress_command_ack(report_id, protocol, reply.as_deref())
    }

    /// Resolve the active Cypress sub-protocol, caching the result. Uses the
    /// value learned from a prior `QS` reply, else queries `M` like the
    /// official app.
    fn cypress_protocol(&self) -> Result<CypressProtocol, Error> {
        if let Some(protocol) = self.cypress_protocol.get() {
            return Ok(protocol);
        }
        let marker = self.cypress_query(report::PROTOCOL_VERSION, b"M\r")?;
        let protocol = Self::parse_cypress_protocol_marker(&marker)?;
        self.cypress_protocol.set(Some(protocol));
        Ok(protocol)
    }

    /// Classify a raw `QS` reply as `V` (ASCII, leading `(`) or `T` (binary)
    /// and cache it for later command dispatch.
    fn classify_cypress_qs(&self, raw: &[u8]) -> CypressProtocol {
        let protocol = if raw.first() == Some(&b'(') {
            CypressProtocol::V
        } else {
            CypressProtocol::T
        };
        self.cypress_protocol.set(Some(protocol));
        protocol
    }

    fn parse_cypress_protocol_marker(marker: &str) -> Result<CypressProtocol, Error> {
        match marker.trim() {
            "V" => Ok(CypressProtocol::V),
            "T" => Ok(CypressProtocol::T),
            other => Err(Error::Parse {
                report_id: report::PROTOCOL_VERSION,
                detail: format!("unknown Cypress sub-protocol marker {other:?}"),
            }),
        }
    }

    fn cypress_protocol_marker(protocol: CypressProtocol) -> &'static str {
        match protocol {
            CypressProtocol::T => "T",
            CypressProtocol::V => "V",
        }
    }

    fn clean_report_text(raw: &str) -> String {
        raw.trim_matches(|c: char| c.is_ascii_control() || c.is_whitespace())
            .to_owned()
    }

    /// Decide whether a Cypress command succeeded from its sub-protocol and
    /// optional reply. `T` is acknowledged implicitly; `V` accepts no reply
    /// (within the ack timeout) or an `ACK`/`(ACK` payload as success.
    fn cypress_command_ack(
        report_id: u8,
        protocol: CypressProtocol,
        reply: Option<&str>,
    ) -> Result<(), Error> {
        if protocol == CypressProtocol::T {
            return Ok(());
        }
        match reply {
            None => Ok(()),
            Some(resp) => {
                let resp = resp.trim();
                if resp.is_empty() || resp.starts_with("ACK") || resp.starts_with("(ACK") {
                    Ok(())
                } else {
                    Err(Error::NotAcknowledged { report_id })
                }
            }
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
                serial_path: None,
            },
            DeviceInfo {
                vid: CYPRESS_VID,
                pid: CYPRESS_PID,
                bus: 1,
                address: 5,
                transport: UpsTransport::CypressHid,
                serial_path: None,
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

    #[test]
    fn serial_cancel_variants_map_to_generic_cancel() {
        assert_eq!(
            Ups::serial_query_command(report::CANCEL_SHUTDOWN).unwrap(),
            Some(b"C\r".as_slice())
        );
        assert_eq!(
            Ups::serial_query_command(report::CANCEL_SHUTDOWN_RESTORE).unwrap(),
            Some(b"C\r".as_slice())
        );
        assert_eq!(
            Ups::serial_query_command(report::CANCEL_SHUTDOWN_RETURN).unwrap(),
            Some(b"C\r".as_slice())
        );
    }

    #[test]
    fn cypress_t_commands_are_always_acknowledged() {
        // T sub-protocol is fire-and-forget: no reply, or any reply, is success.
        assert!(Ups::cypress_command_ack(report::SHORT_TEST, CypressProtocol::T, None).is_ok());
        assert!(
            Ups::cypress_command_ack(report::SHORT_TEST, CypressProtocol::T, Some("garbage"))
                .is_ok()
        );
    }

    #[test]
    fn cypress_v_command_ack_accepts_ack_and_silence() {
        assert!(Ups::cypress_command_ack(report::SHORT_TEST, CypressProtocol::V, None).is_ok());
        assert!(
            Ups::cypress_command_ack(report::SHORT_TEST, CypressProtocol::V, Some("ACK\r")).is_ok()
        );
        assert!(
            Ups::cypress_command_ack(report::SHORT_TEST, CypressProtocol::V, Some("(ACK")).is_ok()
        );
    }

    #[test]
    fn cypress_v_command_ack_rejects_other_reply() {
        assert!(matches!(
            Ups::cypress_command_ack(report::SHORT_TEST, CypressProtocol::V, Some("NAK")),
            Err(Error::NotAcknowledged { report_id }) if report_id == report::SHORT_TEST
        ));
    }

    #[test]
    fn cypress_protocol_marker_parsing() {
        assert_eq!(
            Ups::parse_cypress_protocol_marker("V\r").unwrap(),
            CypressProtocol::V
        );
        assert_eq!(
            Ups::parse_cypress_protocol_marker("T").unwrap(),
            CypressProtocol::T
        );
        assert!(matches!(
            Ups::parse_cypress_protocol_marker("X"),
            Err(Error::Parse { .. })
        ));
    }

    #[test]
    fn report_metadata_text_strips_control_terminators() {
        assert_eq!(Ups::clean_report_text("V\r"), "V");
        assert_eq!(Ups::clean_report_text("\n\t#2000VA\r\n"), "#2000VA");
        assert!(!Ups::clean_report_text("T\r").contains('\r'));
    }

    #[test]
    fn cypress_protocol_marker_formats_for_status_metadata() {
        assert_eq!(Ups::cypress_protocol_marker(CypressProtocol::T), "T");
        assert_eq!(Ups::cypress_protocol_marker(CypressProtocol::V), "V");
    }
}
