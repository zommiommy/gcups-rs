//! GreenCell UPS driver library.
//!
//! Communicates with GreenCell UPS devices over USB HID.
//!
//! Supported transports:
//! - MEC0003 descriptor transport (`0001:0000`), where USB string descriptor
//!   indices act as commands.
//! - Cypress HID Megatec/Q1 transport (`0665:5161`), where ASCII Megatec
//!   commands are sent through HID reports.
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
