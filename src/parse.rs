use crate::error::Error;
use crate::status::{NominalParams, UpsStatus};
use crate::wire::{ACK_RESPONSE, report};

const BATTERY_V_LOW_FACTOR: f64 = 0.915;
const BATTERY_V_HIGH_FACTOR: f64 = 1.05;
/// Online (double-conversion) UPS reports battery voltage through a
/// parallel charging circuit; divide by this to get the true value.
const ONLINE_PARALLEL_DIVISOR: f64 = 2.0;

/// Parse nominal parameters from report F.
/// Format: `#230.0 002 12.00 50.0\r`
pub(crate) fn parse_nominal(raw: &str) -> Result<NominalParams, Error> {
    let raw = raw.trim();
    if raw == ACK_RESPONSE {
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
pub(crate) fn parse_current(raw: &str, nominal: NominalParams) -> Result<UpsStatus, Error> {
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
        // 24V nominal -> low = 21.96, high = 25.20
        assert_eq!(battery_level(21.0, 24.0), 0);
        assert_eq!(battery_level(30.0, 24.0), 100);
        assert_eq!(battery_level(23.58, 24.0), 50); // midpoint
    }
}
