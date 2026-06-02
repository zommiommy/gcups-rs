use core::time::Duration;

use gcups::{DeviceInfo, DeviceSelector, ShutdownDelay, UpsTransport};

#[test]
fn shutdown_delay_lookup() {
    // Descriptor grid: 30/35/40/.../120 s.
    let sd = ShutdownDelay::from_duration(Duration::from_secs(45), UpsTransport::Descriptor);
    assert_eq!(sd.actual_delay(), Duration::from_secs(40));

    let sd = ShutdownDelay::from_duration(Duration::from_secs(120), UpsTransport::Descriptor);
    assert_eq!(sd.actual_delay(), Duration::from_secs(120));

    let sd = ShutdownDelay::from_duration(Duration::from_secs(5), UpsTransport::Descriptor);
    assert_eq!(sd.actual_delay(), Duration::from_secs(30)); // rounds up to smallest step

    // Cypress/Megatec grid is different: 12/18/24/.../60 s.
    let sd = ShutdownDelay::from_duration(Duration::from_secs(45), UpsTransport::CypressHid);
    assert_eq!(sd.actual_delay(), Duration::from_secs(42));

    let sd = ShutdownDelay::from_duration(Duration::from_secs(5), UpsTransport::CypressHid);
    assert_eq!(sd.actual_delay(), Duration::from_secs(12)); // rounds up to smallest step

    let sd = ShutdownDelay::from_duration(Duration::from_secs(30), UpsTransport::ProlificSerial);
    assert_eq!(sd.actual_delay(), Duration::from_secs(0));
    let sd = ShutdownDelay::from_duration(Duration::from_secs(130), UpsTransport::ProlificSerial);
    assert_eq!(sd.actual_delay(), Duration::from_secs(120));
}

#[test]
fn device_selector_parses_vid_pid() {
    let selector: DeviceSelector = "0665:5161".parse().unwrap();
    assert_eq!(selector, DeviceSelector::new(0x0665, 0x5161));
    assert_eq!(selector.to_string(), "0665:5161");
}

#[test]
fn device_selector_parses_location() {
    let selector: DeviceSelector = "0665:5161@001:004".parse().unwrap();
    assert_eq!(
        selector,
        DeviceSelector::with_location(0x0665, 0x5161, 1, 4)
    );
    assert_eq!(selector.to_string(), "0665:5161@001:004");
}

#[test]
fn device_selector_parses_serial_port() {
    let selector: DeviceSelector = "067b:2303@COM4".parse().unwrap();
    assert_eq!(
        selector,
        DeviceSelector::with_serial_path(0x067b, 0x2303, "COM4")
    );
    assert_eq!(selector.to_string(), "067b:2303@COM4");
}

#[test]
fn device_info_returns_copy_pasteable_selector() {
    let device = DeviceInfo {
        vid: 0x0665,
        pid: 0x5161,
        bus: 1,
        address: 4,
        transport: UpsTransport::CypressHid,
        serial_path: None,
    };

    assert_eq!(device.selector().to_string(), "0665:5161@001:004");
    assert_eq!(device.transport.to_string(), "Cypress HID GreenCell QS");
}

#[test]
fn serial_device_info_returns_port_selector() {
    let device = DeviceInfo {
        vid: 0x067b,
        pid: 0x2303,
        bus: 0,
        address: 0,
        transport: UpsTransport::ProlificSerial,
        serial_path: Some("COM4".to_owned()),
    };

    assert_eq!(device.selector().to_string(), "067b:2303@COM4");
    assert_eq!(device.transport.to_string(), "Prolific serial Q1");
}

#[test]
fn device_selector_parses_unpadded_location() {
    let selector: DeviceSelector = "0665:5161@1:4".parse().unwrap();
    assert_eq!(
        selector,
        DeviceSelector::with_location(0x0665, 0x5161, 1, 4)
    );
}

#[test]
fn device_selector_rejects_out_of_range_location() {
    assert!("0665:5161@1:999".parse::<DeviceSelector>().is_err());
}
