//! GreenCell UPS driver library.
//!
//! Communicates with GreenCell UPS devices over USB HID or USB serial.
//!
//! Supported transports:
//! - MEC0003 descriptor HID (`0001:0000`, and the app's alternate `09d6:0001`)
//! - Cypress HID GreenCell QS (`0665:5161`)
//! - Prolific serial Q1 (`067b:2303`, UPS17)
//!
//! `Ups::open()` auto-opens only when exactly one supported UPS is connected.
//! Use [`Ups::list_devices`] and [`Ups::open_with_selector`] when more than one
//! supported UPS is attached.
//!
//! # Quick start
//!
//! ```no_run
//! let ups = gcups::Ups::open()?;
//! let status = ups.status()?;
//! println!("Battery: {}%, on mains: {}", status.battery_level, !status.utility_fail);
//! # Ok::<(), gcups::Error>(())
//! ```

mod device;
mod error;
mod parse;
mod shutdown;
mod status;
mod ups;
mod wire;

pub use device::{DeviceInfo, DeviceLocation, DeviceSelector, UpsTransport};
pub use error::Error;
pub use shutdown::ShutdownDelay;
pub use status::{NominalParams, UpsStatus};
pub use ups::Ups;
