use crate::error::Error;

pub(crate) const MEC_VID: u16 = 0x0001;
pub(crate) const MEC_PID: u16 = 0x0000;
pub(crate) const MEC_ALT_VID: u16 = 0x09d6;
pub(crate) const MEC_ALT_PID: u16 = 0x0001;
pub(crate) const CYPRESS_VID: u16 = 0x0665;
pub(crate) const CYPRESS_PID: u16 = 0x5161;
pub(crate) const PROLIFIC_VID: u16 = 0x067b;
pub(crate) const PROLIFIC_PID: u16 = 0x2303;

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
/// wValue: feature report, report ID 0. Used as a fallback when the device
/// rejects the output report (matches the official app's retry path).
pub(crate) const CYPRESS_FEATURE_REPORT: u16 = 0x03 << 8;
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
    pub(crate) const CURRENT_PARAMS: u8 = 0x03; // Q1 / QS
    pub(crate) const INFO: u8 = 0x0c; // I / M
    pub(crate) const NOMINAL_PARAMS: u8 = 0x0d; // F

    // Commands
    pub(crate) const SHORT_TEST: u8 = 0x04; // T
    pub(crate) const LONG_TEST: u8 = 0x05; // TL / T
    pub(crate) const BEEPER_TOGGLE: u8 = 0x07; // Q
    pub(crate) const SHUTDOWN: u8 = 0x08; // S
    pub(crate) const CANCEL_SHUTDOWN: u8 = 0x0a; // C
    pub(crate) const CANCEL_TEST: u8 = 0x0b; // CT / C
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
        report::PROTOCOL | report::PROTOCOL_VERSION | report::INFO => CypressReport {
            command: b"M\r",
            expects_reply: true,
        },
        report::CURRENT_PARAMS => CypressReport {
            command: b"QS\r",
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
            command: b"T\r",
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
            command: b"C\r",
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

/// Byte-faithful descriptor payload: the low byte of each UTF-16LE code unit
/// after the 2-byte header, returned verbatim.
///
/// Unlike [`decode_string_descriptor`], NUL bytes and bytes `>= 0x80` are kept
/// exactly as received instead of being dropped or remapped to multi-byte
/// UTF-8. Binary reports therefore survive intact, so a raw report dump
/// mirrors the wire instead of corrupting it.
pub(crate) fn descriptor_payload(raw: &[u8]) -> Vec<u8> {
    const HEADER_LEN: usize = 2;
    raw.get(HEADER_LEN..)
        .unwrap_or(&[])
        .iter()
        .copied()
        .step_by(2) // low byte of each UTF-16LE code unit
        .collect()
}

/// Byte-faithful response payload: every byte up to and including the first
/// carriage return, returned verbatim.
///
/// Unlike [`decode_ascii_response`], NUL bytes and bytes `>= 0x80` are
/// preserved, so a binary reply (e.g. the Cypress `T` `QS` frame) is dumped
/// exactly as the device sent it.
pub(crate) fn response_payload(raw: &[u8]) -> &[u8] {
    let end = raw
        .iter()
        .position(|&b| b == b'\r')
        .map_or(raw.len(), |i| i + 1);
    &raw[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cypress_report_mapping_uses_megatec_commands() {
        let current = cypress_report(report::CURRENT_PARAMS).unwrap();
        assert_eq!(current.command, b"QS\r");
        assert!(current.expects_reply);

        let nominal = cypress_report(report::NOMINAL_PARAMS).unwrap();
        assert_eq!(nominal.command, b"F\r");
        assert!(nominal.expects_reply);

        let long_test = cypress_report(report::LONG_TEST).unwrap();
        assert_eq!(long_test.command, b"T\r");
        assert!(!long_test.expects_reply);

        let cancel_test = cypress_report(report::CANCEL_TEST).unwrap();
        assert_eq!(cancel_test.command, b"C\r");
        assert!(!cancel_test.expects_reply);

        let cancel = cypress_report(report::CANCEL_SHUTDOWN_RESTORE).unwrap();
        assert_eq!(cancel.command, b"C\r");
        assert!(!cancel.expects_reply);

        let protocol = cypress_report(report::PROTOCOL).unwrap();
        assert_eq!(protocol.command, b"M\r");
        assert!(protocol.expects_reply);
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

    #[test]
    fn descriptor_payload_is_byte_faithful() {
        // Low bytes carry a NUL (0x00) and a high byte (0xf6); both must survive
        // verbatim, while the text decode drops the NUL and balloons 0xf6 into
        // two UTF-8 bytes.
        let raw: &[u8] = &[8, 3, 0x23, 0, 0x00, 0, 0xf6, 0];
        assert_eq!(descriptor_payload(raw), vec![0x23, 0x00, 0xf6]);
        assert_eq!(decode_string_descriptor(raw), "#\u{f6}");
        assert_eq!(descriptor_payload(&[]), Vec::<u8>::new());
        assert_eq!(descriptor_payload(&[46, 3]), Vec::<u8>::new());
    }

    #[test]
    fn response_payload_preserves_binary_cypress_t_frame() {
        // Documented Cypress T QS frame: contains four NULs and high bytes. The
        // lossy ASCII decode behind the old `raw` path corrupts it; the raw
        // payload must return it verbatim.
        let frame: &[u8] = &[
            b'#', 0x75, 0x01, b' ', 0x6c, b' ', 0x00, 0x01, b' ', 0x6c, b' ', 0x00, b' ', 0x60,
            0x0b, b' ', 0x12, 0xc0, 0x00, b' ', 0xe6, b' ', 0x1e, b' ', 0x0b, b' ', 0x03, b'\r',
        ];
        assert_eq!(response_payload(frame), frame);
        // Stops at the first carriage return, keeping trailing junk out.
        assert_eq!(response_payload(b"#\0\x01\xc0\rtail"), b"#\0\x01\xc0\r");
        // The old text path drops the NULs, so its byte length no longer matches.
        assert_ne!(decode_ascii_response(frame).into_bytes().len(), frame.len());
    }
}
