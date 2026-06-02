# GreenCell UPS USB Protocols

Protocol documentation for GreenCell UPS USB interfaces supported by this
project. The application protocol is Megatec/Q1; supported devices differ
mainly in how those commands are transported over USB.

## Supported device transports

| Transport | Vendor ID | Product ID | USB behavior |
|-----------|-----------|------------|--------------|
| MEC0003 descriptor | `0x0001` | `0x0000` | String descriptor indices act as commands |
| Cypress HID Megatec/Q1 | `0x0665` | `0x5161` | ASCII Megatec commands over HID reports |

Two USB transports are supported:

- **MEC0003 descriptor transport**: the UPS abuses standard USB
  `GET_DESCRIPTOR(STRING)` requests. Each "report" is a USB string descriptor
  at a specific index. Reading a descriptor either returns data or performs a
  side effect and returns an acknowledgement.
- **Cypress HID Megatec/Q1 transport**: the UPS receives normal Megatec ASCII
  commands (`Q1\r`, `F\r`, `I\r`, `T\r`, etc.) through HID output reports and
  returns ASCII replies through interrupt IN packets.

### MEC0003 control transfer parameters

| Field          | Value                              |
|----------------|------------------------------------|
| bmRequestType  | `0x80` (IN, Standard, Device)      |
| bRequest       | `0x06` (GET_DESCRIPTOR)            |
| wValue         | `0x0300 \| report_id`              |
| wIndex         | `0x0000` (interface 0)             |
| wLength        | 96 bytes                           |

The `0x03` in wValue's high byte is the USB descriptor type for STRING.
The low byte is the string descriptor index, which the UPS firmware
interprets as an instruction opcode.

### Cypress HID transfer parameters

For `0665:5161`, commands are sent as 8-byte zero-padded HID output reports:

| Field          | Value                              |
|----------------|------------------------------------|
| bmRequestType  | `0x21` (OUT, Class, Interface)     |
| bRequest       | `0x09` (SET_REPORT)                |
| wValue         | `0x0200` (Output report, ID 0)     |
| wIndex         | `0x0000` (interface 0)             |
| packet size    | 8 bytes                            |

Replies are read from interrupt endpoint `0x81` in 8-byte chunks until `\r`.

### Response format

MEC0003 responses are standard USB string descriptors (UTF-16LE with a 2-byte
header):

```
Byte 0: bLength         (total descriptor length)
Byte 1: bDescriptorType (always 0x03)
Byte 2+: UTF-16LE payload
```

To decode: skip the 2-byte header, take the low byte of each UTF-16LE code unit,
discard null bytes. The result is an ASCII string.

Cypress responses are already ASCII bytes split across 8-byte interrupt packets.

### Command acknowledgement

MEC0003 descriptor commands return the string `UPS No Ack` on success. Any
other response indicates the command was not understood.

Cypress/Megatec commands often return no reply on success. If they do return a
reply, `ACK` or `(ACK` indicates success; an echoed command indicates failure.

## Logical reports and commands

The Rust API uses the following logical report IDs. On MEC0003, these are USB
string descriptor indices. On Cypress HID, supported entries map to Megatec
ASCII commands.

### Queries

| Mnemonic | Logical ID | Cypress command | Description              |
|----------|------------|-----------------|--------------------------|
| -        | `0x01`     | unsupported     | MEC protocol identifier  |
| -        | `0x02`     | unsupported     | MEC protocol version     |
| Q1       | `0x03`     | `Q1\r`          | Current (live) parameters |
| I        | `0x0c`     | `I\r`           | Device info string       |
| F        | `0x0d`     | `F\r`           | Nominal (rated) parameters |

### Commands

| Mnemonic | Logical ID | Cypress command | Description                   |
|----------|------------|-----------------|-------------------------------|
| T        | `0x04`     | `T\r`           | Start short self-test (~10 s) |
| TL       | `0x05`     | `TL\r`          | Start long self-test (~10 min) |
| Q        | `0x07`     | `Q\r`           | Toggle beeper on/off          |
| S        | `0x08`     | generated       | Shutdown                      |
| C        | `0x0a`     | `C\r`           | Cancel shutdown / wake up     |
| CT       | `0x0b`     | `CT\r`          | Cancel self-test              |
| SR       | `0x10`     | generated       | Shutdown with restore         |
| CSR      | `0x1a`     | `C\r`           | Cancel shutdown-and-restore   |
| CS       | `0x2a`     | `C\r`           | Cancel shutdown-return        |
| *        | see table  | generated       | Timed shutdown variants       |

## Nominal parameters (report F, `0x0d`)

Returns the UPS's rated specifications.

### Format

```
#<input_voltage> <input_current> <battery_voltage> <input_frequency>
```

### Example

```
#230.0 008 24.00 50.0
```

| Field             | Example | Unit | Description                     |
|-------------------|---------|------|---------------------------------|
| input_voltage     | 230.0   | V    | Rated mains voltage             |
| input_current     | 008     | A    | Rated input current             |
| battery_voltage   | 24.00   | V    | Nominal battery pack voltage    |
| input_frequency   | 50.0    | Hz   | Rated mains frequency           |

The battery voltage indicates the pack configuration:
- 12 V = 1x 12 V battery
- 24 V = 2x 12 V batteries in series
- 48 V = 4x 12 V batteries in series

## Current parameters (report Q1, `0x03`)

Returns live electrical readings and an 8-bit status register.

### Format

```
(<input_v> <fault_v> <output_v> <load%> <freq> <batt_v> <temp> <register>
```

### Examples

```
(228.2 000.5 226.9 017 50.0 27.4 --.- 00001001   mains present
(000.0 238.1 228.0 001 00.0 25.7 --.- 10001001   on battery
```

| Field       | Type   | Unit | Notes                                  |
|-------------|--------|------|----------------------------------------|
| input_v     | float  | V    | Current mains input voltage            |
| fault_v     | float  | V    | Input voltage at last fault            |
| output_v    | float  | V    | Output voltage to the load             |
| load%       | int    | %    | Load as percentage of rated capacity   |
| freq        | float  | Hz   | Current input frequency                |
| batt_v      | float  | V    | Battery voltage (see adjustment below) |
| temp        | float  | C    | Internal temperature, `--.-` if absent |
| register    | 8-bit  | -    | Binary status flags (see below)        |

### Status register

The register is an 8-character binary string, e.g. `00001001`.
Bit 0 is the rightmost character.

| Bit | Name             | Meaning when set (1)                        |
|-----|------------------|---------------------------------------------|
| 0   | beeper           | Audible alarm is active                     |
| 1   | shutdown_active  | A shutdown countdown is in progress         |
| 2   | test_in_progress | Battery self-test is running                |
| 3   | offline          | UPS is line-interactive (offline) topology  |
| 4   | ups_fault        | Internal fault detected                     |
| 5   | bypass_boost     | Bypass or boost/buck mode active            |
| 6   | battery_low      | Battery charge is critically low            |
| 7   | utility_fail     | Mains power has failed (running on battery) |

**Important:** Bit 3 (`offline`) indicates the UPS *topology* (line-interactive
vs. double-conversion), not the power source. A line-interactive UPS always
has this bit set. Use bit 7 (`utility_fail`) to detect actual mains failure.

### Battery voltage adjustment

For online (double-conversion) UPS units (bit 3 = 0), the reported battery
voltage includes the parallel charging circuit. Divide by 2 and multiply by
the nominal battery voltage to get the true value:

```
true_voltage = reported_voltage * (nominal_battery_voltage / 2)
```

For offline / line-interactive units (bit 3 = 1), the reported voltage is
used as-is.

### Battery level calculation

```
low  = 0.915 * nominal_battery_voltage
high = 1.050 * nominal_battery_voltage
level = 100 * (battery_voltage - low) / (high - low)
level = clamp(level, 0, 100)
```

For a 24 V battery pack: low = 21.96 V, high = 25.20 V.

## Device info (report I, `0x0c`)

Returns a padded model string, e.g.:

```
#                2000VA
```

Trim whitespace and the leading `#` to extract the model designation.

## Shutdown delay mapping

Shutdown delay encoding is transport-specific. Both transports select the
greatest supported delay that does not exceed the requested duration.

### MEC0003 descriptor delay mapping

MEC0003 uses fixed report IDs. Each delay has two report IDs: one for
"shutdown and stay off" and one for "shutdown then restore power when mains
returns."

| Delay   | Shutdown report | Restore report |
|---------|-----------------|----------------|
| 30 s    | `0x18`          | `0x10`         |
| 35 s    | `0x28`          | `0x20`         |
| 40 s    | `0x38`          | `0x30`         |
| 47 s    | `0x48`          | `0x40`         |
| 53 s    | `0x58`          | `0x50`         |
| 60 s    | `0x68`          | `0x60`         |
| 2 min   | `0x78`          | `0x70`         |
| 3 min   | `0x88`          | `0x80`         |
| 4 min   | `0x98`          | `0x90`         |
| 5 min   | `0xa8`          | `0xa0`         |
| 6 min   | `0xb8`          | `0xb0`         |
| 7 min   | `0xc8`          | `0xc0`         |
| 8 min   | `0xd8`          | `0xd0`         |
| 9 min   | `0xe8`          | `0xe0`         |

The report ID encodes the delay in its upper nibble. The lower nibble
distinguishes shutdown (`0x_8`) from shutdown-with-restore (`0x_0`).

### Cypress/Megatec delay mapping

Cypress HID devices use standard Megatec shutdown commands. `<n>` selects the
quantized delay from the grid below:

| Delay range | `<n>` value                     |
|-------------|---------------------------------|
| 12-54 s     | `.2`-`.9` (tenths of a minute)  |
| 1-10 min    | `01`-`10` (whole minutes)       |

The Megatec `S` command has two relevant forms, mapped to the two public methods:

- `shutdown_and_restore(delay)` emits the bare `S<n>\r` form. Per the Megatec
  spec the UPS turns its output off after `<n>`, then reconnects ~10 s after
  utility power is recovered - i.e. it restores on mains return.
- `shutdown(delay)` emits the `S<n>R0000\r` form. Because the bare `S<n>` form
  always returns on mains, there is no standard "stay off" command; `R0000`
  ("never restore") is the convention NUT's `blazer`/`nutdrv_qx` drivers use for
  `shutdown.stayoff`. Some firmware ignores `R0000` and restarts anyway - a
  transport limitation, not a driver bug.

## Known device models

The UPS identifies itself through the nominal battery voltage and
current. Known configurations:

| Model      | Name         | Voltage | Current | Battery   | Topology         |
|------------|--------------|---------|---------|-----------|------------------|
| UPS01/06   | PowerProof   | 12 V    | 2 A     | 1x 12 V   | Line-interactive |
| UPS02/07   | PowerProof   | 12 V    | 3 A     | 1x 12 V   | Line-interactive |
| UPS03      | PowerProof   | 24 V    | 4 A     | 2x 12 V   | Line-interactive |
| UPS04      | PowerProof   | 24 V    | 6 A     | 2x 12 V   | Line-interactive |
| UPS05      | PowerProof   | 24 V    | 8 A     | 2x 12 V   | Line-interactive |
| UPS08      | PureWave     | 24 V    | 4 A     | 2x 12 V   | Line-interactive |
| UPS09      | PureWave     | 24 V    | 8 A     | 2x 12 V   | Line-interactive |
| UPS10/17   | Online MPII  | 24 V    | 4 A     | 2x 12 V   | Online           |
| UPS13      | Rack Tower   | 24 V    | 5 A     | 2x 12 V   | Online           |
| UPS14      | Rack Tower   | 48 V    | 10 A    | 4x 12 V   | Online           |
| UPS15      | Rack Tower   | 72 V    | 13 A    | 6x 12 V   | Online           |

## MEC0003 USB descriptor dump (reference)

Captured from a GreenCell 2000VA line-interactive UPS using the MEC0003
descriptor transport.

### Report F (nominal, index 0x0d)

```
Raw: [46, 3, 35, 0, 50, 0, 51, 0, 48, 0, 46, 0, 48, 0, 32, 0,
      48, 0, 48, 0, 56, 0, 32, 0, 50, 0, 52, 0, 46, 0, 48, 0,
      48, 0, 32, 0, 53, 0, 48, 0, 46, 0, 48, 0, 13, 0]
Decoded: #230.0 008 24.00 50.0
```

### Report Q1 (current, index 0x03)

```
Raw: [96, 3, 40, 0, 50, 0, 50, 0, 56, 0, 46, 0, 50, 0, 32, 0,
      48, 0, 48, 0, 48, 0, 46, 0, 53, 0, 32, 0, 50, 0, 50, 0, ...]
Decoded: (228.2 000.5 226.9 017 50.0 27.4 --.- 00001001
```

### Report I (info, index 0x0c)

```
Decoded: #                2000VA
```
