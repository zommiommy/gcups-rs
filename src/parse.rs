use core::fmt::Write as _;

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

/// Parse the GreenCell Cypress "T" subprotocol response to `QS`.
///
/// The official Electron app decodes this as a stream of mostly-binary bytes:
/// bytes are rendered as hex fields, spaces delimit fields, and `0x28` escapes
/// control bytes. Example decoded frame:
/// `#7501 6c 0001 6c 00 600b 12c000 e6 1e 0b 03\r`.
pub(crate) fn parse_cypress_t_current(raw: &[u8]) -> Result<UpsStatus, Error> {
    let f = parse_cypress_t_fields(raw)?;

    let ab = f[0];
    let c = f[1];
    let de = f[2];
    let f_mult = f[3];
    let load = f[4];
    let hi = f[5];
    let jkl = f[6];
    let m = f[7];
    let n = f[8];
    let reg = f[9] as u8;
    let p = f[10] as u8;

    let nominal = cypress_t_nominal(p)?;
    let input_voltage = (ab * c) as f64 / 51.0 / 256.0;
    let input_voltage_fault = -1.0;
    let output_voltage = (de * f_mult) as f64 / 51.0 / 256.0;
    let load_percent = f64::from(load);
    let input_frequency = jkl as f64 / hi as f64;
    let mut battery_voltage = (m * n) as f64 / 510.0;
    let offline = (reg >> 3) & 1 == 1;

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
        temperature: None,
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

const CYPRESS_T_FIELD_WIDTHS: [usize; 11] = [2, 1, 2, 1, 1, 2, 3, 1, 1, 1, 1];
const CYPRESS_T_FIELD_NAMES: [&str; 11] =
    ["AB", "C", "DE", "F", "G", "HI", "JKL", "M", "N", "O", "P"];

fn parse_cypress_t_fields(raw: &[u8]) -> Result<[u32; 11], Error> {
    if raw.first() != Some(&b'#') {
        return Err(cypress_t_parse_error(format!(
            "missing '#' prefix in Cypress T response {:?}",
            decode_cypress_t(raw)
        )));
    }

    let end = raw.iter().position(|&b| b == b'\r').unwrap_or(raw.len());
    let mut offset = 1;
    let mut fields = [0u32; 11];

    for (i, (&width, name)) in CYPRESS_T_FIELD_WIDTHS
        .iter()
        .zip(CYPRESS_T_FIELD_NAMES)
        .enumerate()
    {
        if i > 0 {
            offset = consume_cypress_t_separators(raw, offset, name)?;
        }

        let mut value = 0u32;
        for byte_index in 0..width {
            let (byte, next) = read_cypress_t_data_byte(raw, offset, name)?;
            if byte == b' ' || byte == b'\r' {
                return Err(cypress_t_parse_error(format!(
                    "short Cypress T field {name}: separator at byte {byte_index} in {:?}",
                    decode_cypress_t(raw)
                )));
            }
            value = (value << 8) | u32::from(byte);
            offset = next;
        }
        fields[i] = value;
    }

    if offset != end {
        return Err(cypress_t_parse_error(format!(
            "extra Cypress T bytes before terminator at offset {offset} in {:?}",
            decode_cypress_t(raw)
        )));
    }

    Ok(fields)
}

fn consume_cypress_t_separators(
    raw: &[u8],
    mut offset: usize,
    next_field: &str,
) -> Result<usize, Error> {
    let start = offset;

    loop {
        match raw.get(offset) {
            Some(b' ') => offset += 1,
            Some(0x28) if raw.get(offset + 1) == Some(&0x04) => offset += 2,
            Some(0x28) if raw.get(offset + 1) == Some(&b' ') => offset += 2,
            _ => break,
        }
    }

    if offset == start {
        return Err(cypress_t_parse_error(format!(
            "missing Cypress T separator before field {next_field} in {:?}",
            decode_cypress_t(raw)
        )));
    }

    Ok(offset)
}

fn read_cypress_t_data_byte(raw: &[u8], offset: usize, field: &str) -> Result<(u8, usize), Error> {
    let Some(&byte) = raw.get(offset) else {
        return Err(cypress_t_parse_error(format!(
            "short Cypress T response while reading field {field} in {:?}",
            decode_cypress_t(raw)
        )));
    };

    if byte != 0x28 {
        return Ok((byte, offset + 1));
    }

    let Some(&escaped) = raw.get(offset + 1) else {
        return Err(cypress_t_parse_error(format!(
            "truncated Cypress T escape while reading field {field} in {:?}",
            decode_cypress_t(raw)
        )));
    };

    let value = match escaped {
        0 => 0x0d,
        1 => 0x11,
        2 => 0x13,
        3 => 0x0a,
        4 => 0x20,
        _ => escaped,
    };
    Ok((value, offset + 2))
}

fn cypress_t_parse_error(detail: String) -> Error {
    Error::Parse {
        report_id: report::CURRENT_PARAMS,
        detail,
    }
}
fn decode_cypress_t(raw: &[u8]) -> String {
    let end = raw
        .iter()
        .position(|&b| b == b'\r')
        .map_or(raw.len(), |i| i + 1);
    let mut out = String::new();
    let mut prev = None;

    for &c in &raw[..end] {
        if c == 0x28 && prev != Some(0x28) {
            prev = Some(c);
            continue;
        }

        match c {
            b' ' => out.push(' '),
            b'#' if out.is_empty() => out.push('#'),
            b'\r' => out.push('\r'),
            _ => {
                let value = if prev == Some(0x28) {
                    match c {
                        0 => 0x0d,
                        1 => 0x11,
                        2 => 0x13,
                        3 => 0x0a,
                        4 => 0x20,
                        _ => c,
                    }
                } else {
                    c
                };
                let _ = write!(out, "{value:02x}");
            }
        }
        prev = Some(c);
    }

    out
}

fn cypress_t_nominal(p: u8) -> Result<NominalParams, Error> {
    let input_voltage = match p & 7 {
        0 => 110.0,
        1 => 120.0,
        2 => 220.0,
        3 => 230.0,
        4 => 240.0,
        _ => {
            return Err(Error::Parse {
                report_id: report::CURRENT_PARAMS,
                detail: format!("unsupported Cypress T nominal input-voltage selector 0x{p:02x}"),
            });
        }
    };

    let battery_voltage = match (p >> 5) & 3 {
        0 => 12.0,
        1 => 24.0,
        2 => 36.0,
        3 => 48.0,
        _ => unreachable!(),
    };

    let input_frequency = if (p >> 7) & 1 == 0 { 50.0 } else { 60.0 };

    Ok(NominalParams {
        input_voltage,
        input_current: -1.0,
        battery_voltage,
        input_frequency,
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
    fn parse_cypress_t_current_official_example() {
        // Official GCUPS 1.1.11 example:
        // UPSLM360 #7501 6c 0001 6c 00 600b 12c000 e6 1e 0b 03
        let raw = [
            b'#', 0x75, 0x01, b' ', 0x6c, b' ', 0x00, 0x01, b' ', 0x6c, b' ', 0x00, b' ', 0x60,
            0x0b, b' ', 0x12, 0xc0, 0x00, b' ', 0xe6, b' ', 0x1e, b' ', 0x0b, b' ', 0x03, b'\r',
        ];
        let s = parse_cypress_t_current(&raw).unwrap();
        assert_eq!(s.nominal.input_voltage, 230.0);
        assert_eq!(s.nominal.input_current, -1.0);
        assert_eq!(s.nominal.battery_voltage, 12.0);
        assert_eq!(s.nominal.input_frequency, 50.0);
        assert!((s.input_voltage - 247.78).abs() < 0.01);
        assert!((s.input_frequency - 49.98).abs() < 0.01);
        assert!((s.battery_voltage - 13.53).abs() < 0.01);
        assert_eq!(s.temperature, None);
        assert!(s.beeper_on);
        assert!(s.shutdown_active);
        assert!(s.offline);
        assert!(!s.utility_fail);
        assert_eq!(s.battery_level, 100);
    }

    #[test]
    fn battery_level_boundaries() {
        // 24V nominal -> low = 21.96, high = 25.20
        assert_eq!(battery_level(21.0, 24.0), 0);
        assert_eq!(battery_level(30.0, 24.0), 100);
        assert_eq!(battery_level(23.58, 24.0), 50); // midpoint
    }

    #[test]
    fn parse_cypress_t_current_handles_escaped_field_separator() {
        // Some Cypress T units escape a separator as `0x28 0x04` (`0x20`) and
        // may also leave an adjacent literal space. The old whitespace parser
        // treated the escaped byte as part of `DE`, yielding a five-digit output
        // voltage. The fixed parser uses field widths and treats it as a
        // separator.
        let raw = [
            b'#', 0x6b, 0x01, b' ', 0x6c, b' ', 0x6b, 0x01, 0x28, 0x04, b' ', 0x6c, b' ', 0x00,
            b' ', 0x5f, 0xf6, b' ', 0x13, 0x12, 0xd0, b' ', 0xdd, b' ', 0x3c, b' ', 0x09, b' ',
            0x23, b'\r',
        ];
        let s = parse_cypress_t_current(&raw).unwrap();
        assert!((s.output_voltage - 226.60).abs() < 0.01);
        assert!((s.battery_voltage - 26.0).abs() < 0.01);
    }

    #[test]
    fn parse_cypress_t_current_rejects_extra_non_separator_bytes() {
        let raw = [
            b'#', 0x6b, 0x01, b' ', 0x6c, b' ', 0x6b, 0x01, 0x7f, b' ', 0x6c, b' ', 0x00, b' ',
            0x5f, 0xf6, b' ', 0x13, 0x12, 0xd0, b' ', 0xdd, b' ', 0x3c, b' ', 0x09, b' ', 0x23,
            b'\r',
        ];
        assert!(parse_cypress_t_current(&raw).is_err());
    }

    #[test]
    fn raw_report_dump_round_trips_through_t_parser() {
        use crate::wire::{decode_ascii_response, response_payload};
        // The documented Cypress T QS frame (NULs + high bytes), as it arrives on
        // the wire.
        let frame: &[u8] = &[
            b'#', 0x75, 0x01, b' ', 0x6c, b' ', 0x00, 0x01, b' ', 0x6c, b' ', 0x00, b' ', 0x60,
            0x0b, b' ', 0x12, 0xc0, 0x00, b' ', 0xe6, b' ', 0x1e, b' ', 0x0b, b' ', 0x03, b'\r',
        ];
        // Byte-faithful `raw` output stays decodable as a real status frame.
        assert!(parse_cypress_t_current(response_payload(frame)).is_ok());
        // The old text-decoding `raw` path dropped NULs and re-encoded high bytes
        // to UTF-8, corrupting the frame past recovery.
        let mangled = decode_ascii_response(frame).into_bytes();
        assert!(parse_cypress_t_current(&mangled).is_err());
    }
}
