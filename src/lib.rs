//! GreenCell UPS driver library.
//!
//! Communicates with GreenCell UPS devices (MEC0003) over USB HID.
//! The UPS exposes its telemetry and accepts commands through USB string
//! descriptors — reading a descriptor at a specific index either returns
//! data or triggers an action.
//!
//! Protocol reverse-engineered from the [gcups](https://github.com/fajfer/gcups) Electron app.
//!
//! # Quick start
//!
//! ```no_run
//! let ups = gcups::Ups::open()?;
//! let status = ups.status()?;
//! println!("Battery: {}%, on mains: {}", status.battery_level, !status.utility_fail);
//! # Ok::<(), gcups::Error>(())
//! ```

use std::fmt;
use std::time::Duration;

use rusb::{Context, DeviceHandle, UsbContext};
use serde::Serialize;
use thiserror::Error;

// ── USB wire constants ──────────────────────────────────────────────────────

const VID: u16 = 0x0001;
const PID: u16 = 0x0000;

/// bmRequestType: IN | Standard | Device.
const BM_REQUEST_TYPE: u8 = 0x80;
/// bRequest: GET_DESCRIPTOR.
const B_REQUEST: u8 = 0x06;
/// wValue high byte: descriptor type 3 (STRING).
const DESC_TYPE_STRING: u16 = 0x03 << 8;
/// wIndex: interface 0.
const W_INDEX: u16 = 0x00;
/// Maximum descriptor payload the device returns.
const BUF_SIZE: usize = 96;

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(3000);

/// Response the device returns when a command is acknowledged.
const ACK_RESPONSE: &str = "UPS No Ack";

// ── Report IDs / instruction opcodes ────────────────────────────────────────

/// Report IDs for the MEC0003 protocol.
///
/// Reading a string descriptor at one of these indices either returns
/// data (queries) or triggers an action (commands) on the UPS.
mod report {
    // Queries
    pub const PROTOCOL: u8 = 0x01;
    pub const PROTOCOL_VERSION: u8 = 0x02;
    pub const CURRENT_PARAMS: u8 = 0x03; // Q1
    pub const INFO: u8 = 0x0c; // I
    pub const NOMINAL_PARAMS: u8 = 0x0d; // F

    // Commands
    pub const SHORT_TEST: u8 = 0x04; // T
    pub const LONG_TEST: u8 = 0x05; // TL
    pub const BEEPER_TOGGLE: u8 = 0x07; // Q
    pub const CANCEL_SHUTDOWN: u8 = 0x0a; // C
    pub const CANCEL_TEST: u8 = 0x0b; // CT
    pub const CANCEL_SHUTDOWN_RESTORE: u8 = 0x1a; // CSR
    pub const CANCEL_SHUTDOWN_RETURN: u8 = 0x2a; // CS
}

// ── Battery voltage thresholds ──────────────────────────────────────────────

const BATTERY_V_LOW_FACTOR: f64 = 0.915;
const BATTERY_V_HIGH_FACTOR: f64 = 1.05;
/// Online (double-conversion) UPS reports battery voltage through a
/// parallel charging circuit; divide by this to get the true value.
const ONLINE_PARALLEL_DIVISOR: f64 = 2.0;

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum Error {
    #[error("UPS not found (VID={VID:04x}, PID={PID:04x}). Is it plugged in?")]
    DeviceNotFound,

    #[error("USB error: {0}")]
    Usb(#[from] rusb::Error),

    #[error("UPS did not acknowledge request for report 0x{report_id:02x}")]
    NotAcknowledged { report_id: u8 },

    #[error("Response too short for report 0x{report_id:02x}: {len} bytes")]
    ResponseTooShort { report_id: u8, len: usize },

    #[error("Parse error for report 0x{report_id:02x}: {detail}")]
    Parse { report_id: u8, detail: String },
}

// ── Public data types ───────────────────────────────────────────────────────

/// Rated (nominal) parameters — the UPS's design-point specifications.
///
/// Returned by the `F` report (descriptor index 0x0d).
/// Format: `#230.0 008 24.00 50.0`
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NominalParams {
    /// Rated input voltage (V), e.g. 230.0.
    pub input_voltage: f64,
    /// Rated input current (A), e.g. 8.
    pub input_current: f64,
    /// Rated battery voltage (V), e.g. 24.0 for a 2×12 V battery pack.
    pub battery_voltage: f64,
    /// Rated input frequency (Hz), e.g. 50.0.
    pub input_frequency: f64,
}

/// Live UPS status — electrical readings and decoded status flags.
///
/// Returned by combining the `F` and `Q1` reports.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UpsStatus {
    /// Current input (mains) voltage.
    pub input_voltage: f64,
    /// Fault-condition input voltage.
    pub input_voltage_fault: f64,
    /// Current output voltage delivered to the load.
    pub output_voltage: f64,
    /// Load as a percentage of rated capacity.
    pub load_percent: f64,
    /// Current input frequency (Hz).
    pub input_frequency: f64,
    /// Current battery voltage (adjusted for UPS topology).
    pub battery_voltage: f64,
    /// Internal temperature (°C), `None` if sensor absent (`--.-`).
    pub temperature: Option<f64>,

    /// Computed battery charge level (0–100%).
    pub battery_level: u8,

    /// Nominal parameters used for the battery-level calculation.
    pub nominal: NominalParams,

    // ── Status register flags (bit 0 → bit 7) ──────────────────────
    /// Beeper is currently active.
    pub beeper_on: bool,
    /// A shutdown sequence is in progress.
    pub shutdown_active: bool,
    /// A battery self-test is running.
    pub test_in_progress: bool,
    /// UPS topology is offline / line-interactive.
    /// This does **not** mean "running on battery" — see [`utility_fail`](Self::utility_fail).
    pub offline: bool,
    /// UPS has detected an internal fault.
    pub ups_fault: bool,
    /// Bypass or boost mode is active.
    pub bypass_or_boost: bool,
    /// Battery charge is critically low.
    pub battery_low: bool,
    /// Mains power has failed — UPS is running on battery.
    pub utility_fail: bool,
}

impl fmt::Display for UpsStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = if self.utility_fail {
            "BATTERY"
        } else {
            "MAINS"
        };
        let low = if self.battery_low { " [LOW]" } else { "" };
        let fault = if self.ups_fault { " [FAULT]" } else { "" };
        write!(
            f,
            "Power: {source}  Battery: {}%{low}  \
             Load: {}%  Input: {:.1}V  Output: {:.1}V{fault}",
            self.battery_level, self.load_percent, self.input_voltage, self.output_voltage,
        )
    }
}

/// Supported shutdown delays.
///
/// The MEC0003 protocol only accepts a fixed set of delay values.
/// Each variant carries the corresponding report ID for both the
/// "shutdown" and "shutdown-and-restore" commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShutdownDelay {
    delay: Duration,
    shutdown_report: u8,
    restore_report: u8,
}

impl ShutdownDelay {
    /// All supported delay steps, ascending.
    const TABLE: &[ShutdownDelay] = &[
        ShutdownDelay::new(30, 0x18, 0x10),
        ShutdownDelay::new(35, 0x28, 0x20),
        ShutdownDelay::new(40, 0x38, 0x30),
        ShutdownDelay::new(47, 0x48, 0x40),
        ShutdownDelay::new(53, 0x58, 0x50),
        ShutdownDelay::new(60, 0x68, 0x60),
        ShutdownDelay::new(120, 0x78, 0x70),
        ShutdownDelay::new(180, 0x88, 0x80),
        ShutdownDelay::new(240, 0x98, 0x90),
        ShutdownDelay::new(300, 0xa8, 0xa0),
        ShutdownDelay::new(360, 0xb8, 0xb0),
        ShutdownDelay::new(420, 0xc8, 0xc0),
        ShutdownDelay::new(480, 0xd8, 0xd0),
        ShutdownDelay::new(540, 0xe8, 0xe0),
    ];

    const fn new(secs: u64, shutdown: u8, restore: u8) -> Self {
        Self {
            delay: Duration::from_secs(secs),
            shutdown_report: shutdown,
            restore_report: restore,
        }
    }

    /// Select the greatest supported delay that is ≤ `requested`.
    /// Falls back to the smallest delay (30 s) if `requested` is shorter.
    pub fn from_duration(requested: Duration) -> Self {
        let mut best = Self::TABLE[0];
        for &entry in Self::TABLE {
            if entry.delay <= requested {
                best = entry;
            }
        }
        best
    }

    /// The actual delay the UPS will use.
    pub fn actual_delay(&self) -> Duration {
        self.delay
    }
}

impl fmt::Display for ShutdownDelay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}s", self.delay.as_secs())
    }
}

// ── UPS handle ──────────────────────────────────────────────────────────────

/// Handle to an open GreenCell UPS device.
///
/// Created via [`Ups::open`]. All methods perform synchronous USB I/O.
pub struct Ups {
    handle: DeviceHandle<Context>,
    timeout: Duration,
}

impl Ups {
    /// Open the first GreenCell MEC0003 UPS found on the USB bus.
    ///
    /// Automatically detaches the kernel HID driver if necessary.
    /// May require root / appropriate udev rules.
    pub fn open() -> Result<Self, Error> {
        let ctx = Context::new()?;
        let handle = ctx
            .open_device_with_vid_pid(VID, PID)
            .ok_or(Error::DeviceNotFound)?;

        let _ = handle.set_auto_detach_kernel_driver(true);
        // Some backends need an explicit claim; ignore failure.
        let _ = handle.claim_interface(0);

        Ok(Self {
            handle,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Override the USB control transfer timeout (default: 3 000 ms).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    // ── Queries ─────────────────────────────────────────────────────

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

    // ── Commands ────────────────────────────────────────────────────

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
    pub fn shutdown(&self, delay: Duration) -> Result<ShutdownDelay, Error> {
        let sd = ShutdownDelay::from_duration(delay);
        self.send_command(sd.shutdown_report)?;
        Ok(sd)
    }

    /// Schedule a UPS shutdown after `delay`, with automatic power restore.
    ///
    /// The UPS powers off, then restores power once mains returns.
    pub fn shutdown_and_restore(&self, delay: Duration) -> Result<ShutdownDelay, Error> {
        let sd = ShutdownDelay::from_duration(delay);
        self.send_command(sd.restore_report)?;
        Ok(sd)
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

    // ── Low-level ───────────────────────────────────────────────────

    /// Read a raw USB string descriptor at `index`, decoded to ASCII.
    pub fn read_descriptor(&self, index: u8) -> Result<String, Error> {
        let mut buf = [0u8; BUF_SIZE];
        let n = self.handle.read_control(
            BM_REQUEST_TYPE,
            B_REQUEST,
            DESC_TYPE_STRING | index as u16,
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

    /// Send a command (reads the descriptor; success = "UPS No Ack" response).
    fn send_command(&self, report_id: u8) -> Result<(), Error> {
        let resp = self.read_descriptor(report_id)?;
        if resp.trim() == ACK_RESPONSE {
            Ok(())
        } else {
            Err(Error::NotAcknowledged { report_id })
        }
    }
}

impl fmt::Debug for Ups {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ups")
            .field("vid", &format_args!("{VID:04x}"))
            .field("pid", &format_args!("{PID:04x}"))
            .finish()
    }
}

// ── Wire decoding ───────────────────────────────────────────────────────────

/// Decode a USB string descriptor (UTF-16LE with 2-byte header) to ASCII.
///
/// Layout: `[bLength, bDescriptorType(0x03), char0_lo, char0_hi, ...]`.
fn decode_string_descriptor(raw: &[u8]) -> String {
    // Skip the 2-byte header (bLength, bDescriptorType=0x03).
    // JS: report.splice(report[1] - 1) where report[1] == 0x03 → start at index 2.
    let start = raw[1].saturating_sub(1) as usize;
    if start >= raw.len() {
        return String::new();
    }
    raw[start..]
        .iter()
        .step_by(2) // low byte of each UTF-16LE code unit
        .filter(|&&b| b != 0)
        .map(|&b| b as char)
        .collect()
}

// ── Response parsing ────────────────────────────────────────────────────────

/// Parse nominal parameters from report F.
/// Format: `#230.0 002 12.00 50.0\r`
fn parse_nominal(raw: &str) -> Result<NominalParams, Error> {
    let raw = raw.trim();
    if raw.trim() == ACK_RESPONSE {
        return Err(Error::NotAcknowledged {
            report_id: report::NOMINAL_PARAMS,
        });
    }
    let body = raw.strip_prefix('#').ok_or_else(|| Error::Parse {
        report_id: report::NOMINAL_PARAMS,
        detail: format!("missing '#' prefix in {raw:?}"),
    })?;

    let f: Vec<&str> = body.split_whitespace().collect();
    if f.len() != 4 {
        return Err(Error::Parse {
            report_id: report::NOMINAL_PARAMS,
            detail: format!("expected 4 fields, got {} in {raw:?}", f.len()),
        });
    }

    let p = |i: usize, name: &str| -> Result<f64, Error> {
        f[i].parse().map_err(|e| Error::Parse {
            report_id: report::NOMINAL_PARAMS,
            detail: format!("cannot parse {name} ({:?}): {e}", f[i]),
        })
    };

    Ok(NominalParams {
        input_voltage: p(0, "input_voltage")?,
        input_current: p(1, "input_current")?,
        battery_voltage: p(2, "battery_voltage")?,
        input_frequency: p(3, "input_frequency")?,
    })
}

/// Parse current parameters from report Q1 and compute battery level.
/// Format: `(242.4 242.4 242.4 004 50.0 13.6 --.- 00001001\r`
fn parse_current(raw: &str, nominal: NominalParams) -> Result<UpsStatus, Error> {
    let raw = raw.trim();
    if raw == ACK_RESPONSE {
        return Err(Error::NotAcknowledged {
            report_id: report::CURRENT_PARAMS,
        });
    }
    let body = raw.strip_prefix('(').ok_or_else(|| Error::Parse {
        report_id: report::CURRENT_PARAMS,
        detail: format!("missing '(' prefix in {raw:?}"),
    })?;

    let f: Vec<&str> = body.split_whitespace().collect();
    if f.len() != 8 {
        return Err(Error::Parse {
            report_id: report::CURRENT_PARAMS,
            detail: format!("expected 8 fields, got {} in {raw:?}", f.len()),
        });
    }

    let p = |i: usize, name: &str| -> Result<f64, Error> {
        f[i].parse().map_err(|e| Error::Parse {
            report_id: report::CURRENT_PARAMS,
            detail: format!("cannot parse {name} ({:?}): {e}", f[i]),
        })
    };

    let input_voltage = p(0, "input_voltage")?;
    let input_voltage_fault = p(1, "input_voltage_fault")?;
    let output_voltage = p(2, "output_voltage")?;
    let load_percent = p(3, "load_percent")?;
    let input_frequency = p(4, "input_frequency")?;
    let mut battery_voltage = p(5, "battery_voltage")?;
    let temperature = f[6].parse::<f64>().ok();

    let reg = u8::from_str_radix(f[7], 2).map_err(|e| Error::Parse {
        report_id: report::CURRENT_PARAMS,
        detail: format!("cannot parse register ({:?}): {e}", f[7]),
    })?;

    let offline = (reg >> 3) & 1 == 1;

    // Online (double-conversion) UPS: adjust for parallel charging circuit.
    if !offline {
        battery_voltage *= nominal.battery_voltage / ONLINE_PARALLEL_DIVISOR;
    }

    let battery_level = battery_level(battery_voltage, nominal.battery_voltage);

    Ok(UpsStatus {
        input_voltage,
        input_voltage_fault,
        output_voltage,
        load_percent,
        input_frequency,
        battery_voltage,
        temperature,
        battery_level,
        nominal,
        beeper_on: reg & 1 == 1,
        shutdown_active: (reg >> 1) & 1 == 1,
        test_in_progress: (reg >> 2) & 1 == 1,
        offline,
        ups_fault: (reg >> 4) & 1 == 1,
        bypass_or_boost: (reg >> 5) & 1 == 1,
        battery_low: (reg >> 6) & 1 == 1,
        utility_fail: (reg >> 7) & 1 == 1,
    })
}

fn battery_level(voltage: f64, nominal: f64) -> u8 {
    let low = BATTERY_V_LOW_FACTOR * nominal;
    let high = BATTERY_V_HIGH_FACTOR * nominal;
    let pct = 100.0 * (voltage - low) / (high - low);
    pct.clamp(0.0, 100.0).round() as u8
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nominal_typical() {
        let p = parse_nominal("#230.0 008 24.00 50.0\r").unwrap();
        assert_eq!(p.input_voltage, 230.0);
        assert_eq!(p.input_current, 8.0);
        assert_eq!(p.battery_voltage, 24.0);
        assert_eq!(p.input_frequency, 50.0);
    }

    #[test]
    fn parse_nominal_missing_prefix() {
        assert!(parse_nominal("230.0 008 24.00 50.0").is_err());
    }

    #[test]
    fn parse_current_mains_present() {
        let nom = NominalParams {
            input_voltage: 230.0,
            input_current: 8.0,
            battery_voltage: 24.0,
            input_frequency: 50.0,
        };
        let s = parse_current("(228.2 000.5 226.9 017 50.0 27.4 --.- 00001001\r", nom).unwrap();
        assert_eq!(s.input_voltage, 228.2);
        assert_eq!(s.load_percent, 17.0);
        assert_eq!(s.temperature, None);
        assert!(s.beeper_on);
        assert!(s.offline); // line-interactive topology
        assert!(!s.utility_fail); // mains present
        assert!(!s.battery_low);
        assert_eq!(s.battery_level, 100);
    }

    #[test]
    fn parse_current_on_battery() {
        let nom = NominalParams {
            input_voltage: 230.0,
            input_current: 8.0,
            battery_voltage: 24.0,
            input_frequency: 50.0,
        };
        let s = parse_current("(000.0 238.1 228.0 001 00.0 25.7 --.- 10001001\r", nom).unwrap();
        assert_eq!(s.input_voltage, 0.0);
        assert!(s.offline);
        assert!(s.utility_fail); // mains failed
        assert_eq!(s.battery_level, 100); // 25.7V > 25.2V high threshold
    }

    #[test]
    fn parse_current_online_ups() {
        // Simulated online UPS (offline bit = 0), battery voltage adjusted.
        let nom = NominalParams {
            input_voltage: 230.0,
            input_current: 4.0,
            battery_voltage: 24.0,
            input_frequency: 50.0,
        };
        let s = parse_current("(230.0 000.0 230.0 010 50.0 2.10 25.0 00000001\r", nom).unwrap();
        assert!(!s.offline);
        // 2.10 * (24.0 / 2.0) = 25.2
        assert!((s.battery_voltage - 25.2).abs() < 0.01);
        assert_eq!(s.temperature, Some(25.0));
    }

    #[test]
    fn battery_level_boundaries() {
        // 24V nominal → low = 21.96, high = 25.20
        assert_eq!(battery_level(21.0, 24.0), 0);
        assert_eq!(battery_level(30.0, 24.0), 100);
        assert_eq!(battery_level(23.58, 24.0), 50); // midpoint
    }

    #[test]
    fn shutdown_delay_lookup() {
        let sd = ShutdownDelay::from_duration(Duration::from_secs(45));
        assert_eq!(sd.actual_delay(), Duration::from_secs(40));

        let sd = ShutdownDelay::from_duration(Duration::from_secs(120));
        assert_eq!(sd.actual_delay(), Duration::from_secs(120));

        // Below minimum clamps to 30s.
        let sd = ShutdownDelay::from_duration(Duration::from_secs(5));
        assert_eq!(sd.actual_delay(), Duration::from_secs(30));
    }

    #[test]
    fn string_descriptor_decode() {
        // Real capture: report 0x0d → "#230.0 008 24.00 50.0\r"
        let raw: &[u8] = &[
            46, 3, // bLength=46, bDescriptorType=3
            35, 0, 50, 0, 51, 0, 48, 0, 46, 0, 48, 0, 32, 0, 48, 0, 48, 0, 56, 0, 32, 0, 50, 0, 52,
            0, 46, 0, 48, 0, 48, 0, 32, 0, 53, 0, 48, 0, 46, 0, 48, 0, 13, 0,
        ];
        let s = decode_string_descriptor(raw);
        assert_eq!(s, "#230.0 008 24.00 50.0\r");
    }
}
