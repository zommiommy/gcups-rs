use thiserror::Error;

use crate::device::DeviceSelector;

#[derive(Debug, Error)]
pub enum Error {
    #[error(
        "UPS not found (supported VID:PID: 0001:0000, 09d6:0001, 0665:5161, 067b:2303). Is it plugged in?"
    )]
    DeviceNotFound,

    #[error("no UPS matches selector {selector}")]
    DeviceNotFoundBySelector { selector: DeviceSelector },

    #[error(
        "{count} supported UPS devices connected; use `gcups list` and pass `--device <selector>`"
    )]
    AmbiguousDeviceAuto { count: usize },

    #[error(
        "{count} UPS devices match selector {selector}; use the BUS:ADDR selector from `gcups list`"
    )]
    AmbiguousDeviceSelector {
        selector: DeviceSelector,
        count: usize,
    },

    #[error("USB error: {0}")]
    Usb(#[from] rusb::Error),

    #[error("Report 0x{report_id:02x} is not supported by this UPS transport")]
    UnsupportedReport { report_id: u8 },

    #[error("UPS did not acknowledge request for report 0x{report_id:02x}")]
    NotAcknowledged { report_id: u8 },

    #[error("Response too short for report 0x{report_id:02x}: {len} bytes")]
    ResponseTooShort { report_id: u8, len: usize },

    #[error("Short USB write for report 0x{report_id:02x}: wrote {len} of {expected} bytes")]
    ShortWrite {
        report_id: u8,
        len: usize,
        expected: usize,
    },

    #[error("Parse error for report 0x{report_id:02x}: {detail}")]
    Parse { report_id: u8, detail: String },

    #[error("Serial error: {detail}")]
    Serial { detail: String },
}
