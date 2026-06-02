use core::fmt;

use serde::Serialize;

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
/// Returned by combining nominal and current data for the active transport.
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
