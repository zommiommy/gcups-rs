use crate::error::Error;

pub(crate) const MEC_VID: u16 = 0x0001;
pub(crate) const MEC_PID: u16 = 0x0000;
pub(crate) const CYPRESS_VID: u16 = 0x0665;
pub(crate) const CYPRESS_PID: u16 = 0x5161;

/// bmRequestType: IN | Standard | Device.
pub(crate) const BM_REQUEST_TYPE: u8 = 0x80;
/// bRequest: GET_DESCRIPTOR.
pub(crate) const B_REQUEST: u8 = 0x06;
/// wValue high byte: descriptor type 3 (STRING).
pub(crate) const DESC_TYPE_STRING: u16 = 0x03 << 8;
/// wIndex: interface 0.
pub(crate) const W_INDEX: u16 = 0x00;
/// Maximum descriptor payload the device returns.
pub(crate) const BUF_SIZE: usize = 96;

/// bmRequestType: OUT | Class | Interface.
pub(crate) const CYPRESS_SET_REPORT_REQUEST_TYPE: u8 = 0x21;
/// bRequest: SET_REPORT.
pub(crate) const CYPRESS_SET_REPORT: u8 = 0x09;
/// wValue: output report, report ID 0.
pub(crate) const CYPRESS_OUTPUT_REPORT: u16 = 0x02 << 8;
pub(crate) const CYPRESS_INTERRUPT_IN: u8 = 0x81;
pub(crate) const CYPRESS_PACKET_SIZE: usize = 8;
pub(crate) const MEGATEC_MAX_COMMAND_LEN: usize = 10;

/// Response the descriptor transport returns when a command is acknowledged.
pub(crate) const ACK_RESPONSE: &str = "UPS No Ack";

/// Report IDs for the MEC0003 protocol.
///
/// Reading a string descriptor at one of these indices either returns data
/// (queries) or triggers an action (commands) on the UPS.
pub(crate) mod report {
    // Queries
    pub(crate) const PROTOCOL: u8 = 0x01;
    pub(crate) const PROTOCOL_VERSION: u8 = 0x02;
    pub(crate) const CURRENT_PARAMS: u8 = 0x03; // Q1
    pub(crate) const INFO: u8 = 0x0c; // I
    pub(crate) const NOMINAL_PARAMS: u8 = 0x0d; // F

    // Commands
    pub(crate) const SHORT_TEST: u8 = 0x04; // T
    pub(crate) const LONG_TEST: u8 = 0x05; // TL
    pub(crate) const BEEPER_TOGGLE: u8 = 0x07; // Q
    pub(crate) const SHUTDOWN: u8 = 0x08; // S
    pub(crate) const CANCEL_SHUTDOWN: u8 = 0x0a; // C
    pub(crate) const CANCEL_TEST: u8 = 0x0b; // CT
    pub(crate) const SHUTDOWN_RESTORE: u8 = 0x10; // SR
    pub(crate) const CANCEL_SHUTDOWN_RESTORE: u8 = 0x1a; // CSR
    pub(crate) const CANCEL_SHUTDOWN_RETURN: u8 = 0x2a; // CS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CypressReport {
    pub(crate) command: &'static [u8],
    pub(crate) expects_reply: bool,
}

pub(crate) fn cypress_report(report_id: u8) -> Result<CypressReport, Error> {
    let report = match report_id {
        report::CURRENT_PARAMS => CypressReport {
            command: b"Q1\r",
            expects_reply: true,
        },
        report::INFO => CypressReport {
            command: b"I\r",
            expects_reply: true,
        },
        report::NOMINAL_PARAMS => CypressReport {
            command: b"F\r",
            expects_reply: true,
        },
        report::SHORT_TEST => CypressReport {
            command: b"T\r",
            expects_reply: false,
        },
        report::LONG_TEST => CypressReport {
            command: b"TL\r",
            expects_reply: false,
        },
        report::BEEPER_TOGGLE => CypressReport {
            command: b"Q\r",
            expects_reply: false,
        },
        report::CANCEL_SHUTDOWN
        | report::CANCEL_SHUTDOWN_RESTORE
        | report::CANCEL_SHUTDOWN_RETURN => CypressReport {
            command: b"C\r",
            expects_reply: false,
        },
        report::CANCEL_TEST => CypressReport {
            command: b"CT\r",
            expects_reply: false,
        },
        _ => return Err(Error::UnsupportedReport { report_id }),
    };
    Ok(report)
}

/// Decode a USB string descriptor (UTF-16LE with 2-byte header) to ASCII.
///
/// Layout: `[bLength, bDescriptorType(0x03), char0_lo, char0_hi, ...]`.
pub(crate) fn decode_string_descriptor(raw: &[u8]) -> String {
    // Skip the 2-byte header (bLength, bDescriptorType); the payload is
    // UTF-16LE, so take the low byte of each code unit and drop nulls.
    const HEADER_LEN: usize = 2;
    if raw.len() <= HEADER_LEN {
        return String::new();
    }
    raw[HEADER_LEN..]
        .iter()
        .step_by(2) // low byte of each UTF-16LE code unit
        .filter(|&&b| b != 0)
        .map(|&b| char::from(b))
        .collect()
}

pub(crate) fn decode_ascii_response(raw: &[u8]) -> String {
    let end = raw
        .iter()
        .position(|&b| b == b'\r')
        .map_or(raw.len(), |i| i + 1);
    raw[..end]
        .iter()
        .filter(|&&b| b != 0)
        .map(|&b| char::from(b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cypress_report_mapping_uses_megatec_commands() {
        let current = cypress_report(report::CURRENT_PARAMS).unwrap();
        assert_eq!(current.command, b"Q1\r");
        assert!(current.expects_reply);

        let nominal = cypress_report(report::NOMINAL_PARAMS).unwrap();
        assert_eq!(nominal.command, b"F\r");
        assert!(nominal.expects_reply);

        let cancel = cypress_report(report::CANCEL_SHUTDOWN_RESTORE).unwrap();
        assert_eq!(cancel.command, b"C\r");
        assert!(!cancel.expects_reply);

        assert!(matches!(
            cypress_report(report::PROTOCOL),
            Err(Error::UnsupportedReport {
                report_id: report::PROTOCOL
            })
        ));
    }

    #[test]
    fn ascii_response_decode_stops_at_carriage_return() {
        let s = decode_ascii_response(b"(ACK\r\0\0ignored");
        assert_eq!(s, "(ACK\r");
    }

    #[test]
    fn string_descriptor_decode() {
        // Real capture: report 0x0d -> "#230.0 008 24.00 50.0\r"
        let raw: &[u8] = &[
            46, 3, // bLength=46, bDescriptorType=3
            35, 0, 50, 0, 51, 0, 48, 0, 46, 0, 48, 0, 32, 0, 48, 0, 48, 0, 56, 0, 32, 0, 50, 0, 52,
            0, 46, 0, 48, 0, 48, 0, 32, 0, 53, 0, 48, 0, 46, 0, 48, 0, 13, 0,
        ];
        let s = decode_string_descriptor(raw);
        assert_eq!(s, "#230.0 008 24.00 50.0\r");
    }

    #[test]
    fn string_descriptor_decode_handles_short_input() {
        // Fewer than 2 header bytes, or a header pointing past the end, must
        // return empty rather than panic on the index.
        assert_eq!(decode_string_descriptor(&[]), "");
        assert_eq!(decode_string_descriptor(&[46]), "");
        assert_eq!(decode_string_descriptor(&[46, 3]), "");
    }
}
