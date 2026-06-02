use core::fmt;
use core::str::FromStr;

use crate::wire::{CYPRESS_PID, CYPRESS_VID, MEC_PID, MEC_VID};

/// USB bus/address selector for one physical UPS instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceLocation {
    pub bus: u8,
    pub address: u8,
}

/// Selector accepted by [`Ups::open_with_selector`](crate::Ups::open_with_selector).
///
/// Format: `VID:PID` or `VID:PID@BUS:ADDR`, where VID/PID are hexadecimal and
/// BUS/ADDR are decimal USB bus and address numbers as printed by `gcups list`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceSelector {
    pub vid: u16,
    pub pid: u16,
    pub location: Option<DeviceLocation>,
}

impl DeviceSelector {
    pub const fn new(vid: u16, pid: u16) -> Self {
        Self {
            vid,
            pid,
            location: None,
        }
    }

    pub const fn with_location(vid: u16, pid: u16, bus: u8, address: u8) -> Self {
        Self {
            vid,
            pid,
            location: Some(DeviceLocation { bus, address }),
        }
    }

    pub(crate) fn matches(&self, device: DeviceInfo) -> bool {
        if self.vid != device.vid || self.pid != device.pid {
            return false;
        }

        match self.location {
            Some(location) => location.bus == device.bus && location.address == device.address,
            None => true,
        }
    }
}

impl fmt::Display for DeviceSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = format!("{:04x}:{:04x}", self.vid, self.pid);
        if let Some(location) = self.location {
            s += &format!("@{:03}:{:03}", location.bus, location.address);
        }
        f.pad(&s)
    }
}

impl FromStr for DeviceSelector {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (id, location) = if let Some((id, location)) = s.split_once('@') {
            (id, Some(location))
        } else {
            (s, None)
        };

        let (vid, pid) = id
            .split_once(':')
            .ok_or_else(|| "expected VID:PID or VID:PID@BUS:ADDR".to_owned())?;

        let vid = parse_hex_u16(vid, "VID")?;
        let pid = parse_hex_u16(pid, "PID")?;
        let Some(location) = location else {
            return Ok(DeviceSelector::new(vid, pid));
        };

        let (bus, address) = location
            .split_once(':')
            .ok_or_else(|| "expected BUS:ADDR after @".to_owned())?;

        Ok(DeviceSelector::with_location(
            vid,
            pid,
            parse_decimal_u8(bus, "bus")?,
            parse_decimal_u8(address, "address")?,
        ))
    }
}

fn parse_hex_u16(s: &str, name: &str) -> Result<u16, String> {
    if s.is_empty() || s.len() > 4 {
        return Err(format!("{name} must be 1 to 4 hexadecimal digits"));
    }
    u16::from_str_radix(s, 16).map_err(|e| format!("invalid {name}: {e}"))
}

fn parse_decimal_u8(s: &str, name: &str) -> Result<u8, String> {
    if s.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    s.parse().map_err(|e| format!("invalid {name}: {e}"))
}

/// USB transport used by a supported UPS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpsTransport {
    Descriptor,
    CypressHid,
}

impl fmt::Display for UpsTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpsTransport::Descriptor => f.write_str("MEC0003 descriptor"),
            UpsTransport::CypressHid => f.write_str("Cypress HID Megatec/Q1"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SupportedDevice {
    pub(crate) vid: u16,
    pub(crate) pid: u16,
    pub(crate) transport: UpsTransport,
}

pub(crate) const SUPPORTED_DEVICES: &[SupportedDevice] = &[
    SupportedDevice {
        vid: MEC_VID,
        pid: MEC_PID,
        transport: UpsTransport::Descriptor,
    },
    SupportedDevice {
        vid: CYPRESS_VID,
        pid: CYPRESS_PID,
        transport: UpsTransport::CypressHid,
    },
];

/// A supported UPS currently visible on the USB bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub vid: u16,
    pub pid: u16,
    pub bus: u8,
    pub address: u8,
    pub transport: UpsTransport,
}

impl DeviceInfo {
    pub fn selector(&self) -> DeviceSelector {
        DeviceSelector::with_location(self.vid, self.pid, self.bus, self.address)
    }
}

pub(crate) fn supported_transport(vid: u16, pid: u16) -> Option<UpsTransport> {
    SUPPORTED_DEVICES
        .iter()
        .find(|device| device.vid == vid && device.pid == pid)
        .map(|device| device.transport)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_device_ids_include_cypress() {
        assert!(SUPPORTED_DEVICES.iter().any(|d| {
            d.vid == CYPRESS_VID && d.pid == CYPRESS_PID && d.transport == UpsTransport::CypressHid
        }));
    }

    #[test]
    fn selector_display_honors_width() {
        // The `list` table pads this column; Display must respect the width
        // specifier (regression: a bare write! ignored it and misaligned).
        let sel = DeviceSelector::with_location(0x0001, 0x0000, 5, 3);
        assert_eq!(sel.to_string(), "0001:0000@005:003");
        let padded = format!("{sel:<22}");
        assert_eq!(padded.len(), 22);
        assert_eq!(padded.trim_end(), "0001:0000@005:003");
    }
}
