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
fn device_info_returns_copy_pasteable_selector() {
    let device = DeviceInfo {
        vid: 0x0665,
        pid: 0x5161,
        bus: 1,
        address: 4,
        transport: UpsTransport::CypressHid,
    };

    assert_eq!(device.selector().to_string(), "0665:5161@001:004");
    assert_eq!(device.transport.to_string(), "Cypress HID Megatec/Q1");
}
