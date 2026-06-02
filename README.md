# gcups

Rust driver and CLI for GreenCell UPS devices using either the MEC0003
descriptor transport or the Cypress HID Megatec/Q1 transport (`0665:5161`).

Communicates over USB HID to read battery status, electrical parameters,
and send commands (shutdown, self-test, beeper toggle, etc.). No Docker,
no Electron, no proprietary runtime -- just a single static binary or a
library you can embed.

The MEC0003 descriptor transport was reverse-engineered from the official
[gcups](https://github.com/fajfer/gcups) Electron app. The Cypress transport
uses the same Megatec/Q1 commands over HID reports.
See [PROTOCOL.md](PROTOCOL.md) for the full wire-level documentation.

## CLI

With no subcommand, `gcups` prints a one-line status for scripting:

```
$ gcups
Power: MAINS  Battery: 100%  Load: 17%  Input: 228.2V  Output: 226.9V
```

Use `gcups status` for a full report:

```
$ gcups status
Device
  Model:              2000VA
  Protocol:           MEC vMEC0003
  Topology:           line-interactive

Mains
  Input voltage:      228.2 V
  Input frequency:    50.0 Hz
  Fault voltage:      0.5 V

Output
  Output voltage:     226.9 V
  Load:               17%
  Temperature:        n/a

Battery
  Level:              100%
  Voltage:            27.4 V
  Pack:               2x 12 V (24 V nominal)
  Low:                no

Status
  Power source:       mains
  Utility fail:       no
  UPS fault:          no
  Bypass/boost:       no
  Beeper:             yes
  Shutdown active:    no
  Test in progress:   no

Rated
  Input voltage:      230.0 V
  Input current:      8 A
  Input frequency:    50.0 Hz
  Battery voltage:    24.0 V
```

### Commands

```
$ gcups                     # one-line status (for scripting)
$ gcups list                # list supported UPS devices and selectors
$ gcups --device 0665:5161@001:004 status
$ gcups status              # full status report
$ gcups status --json       # full JSON output
$ gcups nominal             # rated parameters
$ gcups nominal --json
$ gcups info                # model string (e.g. "2000VA")
$ gcups protocol            # protocol identifier
$ gcups protocol-version    # protocol version
$ gcups raw 0x0d            # raw report read by logical ID
$ gcups test-short          # start ~10 s battery self-test
$ gcups test-long           # start ~10 min battery self-test
$ gcups test-cancel         # cancel running test
$ gcups beeper              # toggle beeper on/off
$ gcups shutdown 60         # shutdown in 60 s (stays off)
$ gcups shutdown-restore 60 # shutdown in 60 s, restore on mains return
$ gcups cancel-shutdown     # cancel pending shutdown
$ gcups watch              # continuous columnar polling (Ctrl-C to stop)
$ gcups watch -i 200ms -d 30s --format csv > trace.csv
$ gcups watch --changes-only --format json   # only emit when register bits flip
$ gcups wakeup              # restore power
```

Without `--device`, auto-detection opens the UPS only when exactly one supported
device is connected. If multiple supported UPSes are connected, run `gcups list`
and pass the printed selector with `--device`. A `VID:PID` selector works only
when one device of that type is present; two UPSes of the same type require the
full `VID:PID@BUS:ADDR` selector.

### Device selection

`gcups list` prints supported devices in the selector format accepted by
`--device`:

```
Selector               VID:PID   USB     Transport
0001:0000@005:003      0001:0000 005:003 MEC0003 descriptor
0665:5161@001:004      0665:5161 001:004 Cypress HID Megatec/Q1
```

Selector forms:

| Form | Meaning |
|------|---------|
| `VID:PID` | Selects a device type; valid only when exactly one matching device is connected |
| `VID:PID@BUS:ADDR` | Selects one physical USB device; required for two UPSes of the same type |

Examples:

```
gcups --device 0665:5161 status
gcups --device 0665:5161@001:004 status
```

`list` does not open or claim the UPS; it only enumerates supported USB devices.

### Exit codes

| Code | Condition                  |
|------|----------------------------|
| 0    | Mains present, battery OK  |
| 1    | Mains failed (on battery)  |
| 2    | Battery low                |
| 3    | UPS fault                  |
| 10   | Device error               |

`gcups` (bare) and `gcups watch` use the same codes; `watch` reports its most
recent sample's condition when it exits via `--count` or `--duration`.

Use the exit code to trigger a safe shutdown:

```bash
gcups status --json
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

### Selecting a device

```rust
let devices = gcups::Ups::list_devices()?;
for device in &devices {
    println!("{} {}", device.selector(), device.transport);
}

let selector = "0665:5161@001:004".parse().expect("valid selector");
let ups = gcups::Ups::open_with_selector(selector)?;
let status = ups.status()?;
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
| `Ups::open()`                   | Auto-open when exactly one UPS is present |
| `Ups::list_devices()`           | List supported connected UPS devices      |
| `Ups::open_with_selector(sel)`  | Open a selected UPS                       |
| `ups.status()`                  | Live readings and status flags            |
| `ups.nominal_params()`          | Rated specifications                      |
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
| `ups.read_descriptor(index)`    | Raw low-level report read                |

## Installation

### NixOS (flake)

Add the input to your `flake.nix`:

```nix
inputs = {
  gcups-rs = {
    url = "github:zommiommy/gcups-rs";
    inputs.nixpkgs.follows = "nixpkgs";
  };
};
```

Then either use the overlay:

```nix
nixpkgs.overlays = [ inputs.gcups-rs.overlays.default ];
environment.systemPackages = [ pkgs.gcups ];
```

Or reference the package directly:

```nix
environment.systemPackages = [
  inputs.gcups-rs.packages.${system}.default
];
```

### Building from source

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
Either run as root or add udev rules for the supported transports:

```
# /etc/udev/rules.d/99-gcups.rules
SUBSYSTEM=="usb", ATTRS{idVendor}=="0001", ATTRS{idProduct}=="0000", MODE="0666"
SUBSYSTEM=="usb", ATTRS{idVendor}=="0665", ATTRS{idProduct}=="5161", MODE="0666"
```

On NixOS:

```nix
services.udev.extraRules = ''
  SUBSYSTEM=="usb", ATTRS{idVendor}=="0001", ATTRS{idProduct}=="0000", MODE="0666"
  SUBSYSTEM=="usb", ATTRS{idVendor}=="0665", ATTRS{idProduct}=="5161", MODE="0666"
'';
```

## Disclaimer

This code was written by an LLM. No assurances are provided. Use at your own risk.

I am not affiliated with the GREENCELL.GLOBAL brand, I don't represent and I was
never employed by CSG S.A. nor was I ever contracted by them for doing any work
whatsoever.