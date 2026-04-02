# gcups

Rust driver and CLI for GreenCell UPS devices (MEC0003).

Communicates over USB HID to read battery status, electrical parameters,
and send commands (shutdown, self-test, beeper toggle, etc.). No Docker,
no Electron, no proprietary runtime -- just a single static binary or a
library you can embed.

Protocol reverse-engineered from the official
[gcups](https://github.com/fajfer/gcups) Electron app.
See [PROTOCOL.md](PROTOCOL.md) for the full wire-level documentation.

## CLI

```
$ gcups
Power: MAINS  Battery: 100%  Load: 17%  Input: 228.2V  Output: 226.9V

$ gcups --json
{
  "input_voltage": 226.9,
  "input_voltage_fault": 0.5,
  "output_voltage": 226.9,
  "load_percent": 17.0,
  "input_frequency": 50.0,
  "battery_voltage": 27.4,
  "temperature": null,
  "battery_level": 100,
  "nominal": {
    "input_voltage": 230.0,
    "input_current": 8.0,
    "battery_voltage": 24.0,
    "input_frequency": 50.0
  },
  "beeper_on": true,
  "shutdown_active": false,
  "test_in_progress": false,
  "offline": true,
  "ups_fault": false,
  "bypass_or_boost": false,
  "battery_low": false,
  "utility_fail": false
}
```

### Exit codes

| Code | Condition                  |
|------|----------------------------|
| 0    | Mains present, battery OK  |
| 1    | Mains failed (on battery)  |
| 2    | Battery low                |
| 3    | UPS fault                  |
| 10   | Device error               |

Use the exit code in a script to trigger a safe shutdown:

```bash
gcups --json
case $? in
  1|2) sync && systemctl poweroff ;;
esac
```

## Library

Add to your `Cargo.toml`:

```toml
[dependencies]
gcups = { path = "../gcups-rs" }
```

### Reading status

```rust
let ups = gcups::Ups::open()?;
let status = ups.status()?;

if status.utility_fail {
    eprintln!("mains power lost, battery at {}%", status.battery_level);
}
```

### Sending commands

```rust
use std::time::Duration;

let ups = gcups::Ups::open()?;

// Battery self-test
ups.short_test()?;

// Schedule shutdown in 60 seconds (with auto-restore on mains return)
let delay = ups.shutdown_and_restore(Duration::from_secs(60))?;
println!("UPS will shut down in {delay}");

// Cancel it
ups.cancel_shutdown_restore()?;

// Toggle beeper
ups.toggle_beeper()?;
```

### Full API

| Method                          | Description                              |
|---------------------------------|------------------------------------------|
| `Ups::open()`                   | Find and open the UPS                    |
| `ups.status()`                  | Live readings and status flags           |
| `ups.nominal_params()`          | Rated specifications                     |
| `ups.device_info()`             | Model string (e.g. "2000VA")             |
| `ups.protocol()`                | Protocol identifier                      |
| `ups.protocol_version()`        | Protocol version                         |
| `ups.short_test()`              | Start ~10 s battery test                 |
| `ups.long_test()`               | Start ~10 min battery test               |
| `ups.cancel_test()`             | Cancel running test                      |
| `ups.toggle_beeper()`           | Toggle beeper on/off                     |
| `ups.shutdown(delay)`           | Shutdown after delay (stays off)         |
| `ups.shutdown_and_restore(delay)` | Shutdown, restore on mains return      |
| `ups.cancel_shutdown()`         | Cancel pending shutdown                  |
| `ups.cancel_shutdown_restore()` | Cancel shutdown-and-restore              |
| `ups.cancel_shutdown_return()`  | Cancel shutdown-return                   |
| `ups.wake_up()`                 | Restore power                            |
| `ups.read_descriptor(index)`    | Raw low-level descriptor read            |

## Building

Requires `libusb-1.0` development headers.

```bash
# NixOS
nix-shell -p pkg-config libusb1 --run 'cargo build --release'

# Debian/Ubuntu
sudo apt install libusb-1.0-0-dev pkg-config
cargo build --release
```

## Permissions

The UPS shows up as a HID device. By default only root can access it.
Either run as root or add a udev rule:

```
# /etc/udev/rules.d/99-gcups.rules
SUBSYSTEM=="usb", ATTRS{idVendor}=="0001", ATTRS{idProduct}=="0000", MODE="0666"
```

On NixOS:

```nix
services.udev.extraRules = ''
  SUBSYSTEM=="usb", ATTRS{idVendor}=="0001", ATTRS{idProduct}=="0000", MODE="0666"
'';
```

## License

MIT
