# GreenCell UPS USB Protocols

Protocol documentation for GreenCell UPS USB interfaces supported by this
project. MEC0003 devices use descriptor-indexed Megatec/Q1 reports; Cypress
devices (`0665:5161`) use GreenCell's QS protocol family (`T` or `V`) over HID.
Prolific devices (`067b:2303`) use Q1 over a 2400 8N1 serial port.

Findings in this document were audited against Green Cell's official GCUPS
Electron app v1.1.11 (`app.asar`). Relevant source files:

- `definitions/mec003.driver.js`
- `definitions/cypress-t.driver.js`
- `definitions/prolific.driver.js`
- `adapter/hid.adapter.js`
- `adapter/usb.adapter.js`
- `adapter/serialport.adapter.js`
- `definitions/protocol/q1/*`
- `definitions/protocol/qs/*`

## Supported device transports

| Transport | Vendor ID | Product ID | Behavior |
|-----------|-----------|------------|----------|
| MEC0003 descriptor HID | `0x0001` | `0x0000` | String descriptor indices act as commands |
| MEC0003 descriptor HID (alt) | `0x09d6` | `0x0001` | Same `Mec003Driver` as `0001:0000` |
| Cypress HID GreenCell QS | `0x0665` | `0x5161` | GreenCell QS commands over HID reports |
| Prolific serial Q1 | `0x067b` | `0x2303` | Q1 commands over 2400 8N1 serial |

Three transport families are supported:

- **MEC0003 descriptor HID**: the UPS abuses standard USB
  `GET_DESCRIPTOR(STRING)` requests. Each "report" is a USB string descriptor
  at a specific index. Reading a descriptor either returns data or performs a
  side effect and returns an acknowledgement.
- **Cypress HID GreenCell QS**: the UPS receives ASCII commands from
  GreenCell's official Cypress driver (`M\r`, `QS\r`, `F\r`, `T\r`, etc.) through
  HID output reports and returns replies through interrupt IN packets.
- **Prolific serial Q1**: the UPS receives ASCII Q1-family commands (`Q1\r`,
  `F\r`, `I\r`, `TL\r`, etc.) over a 2400 baud, 8 data bits, no parity,
  1 stop bit serial port.

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

For `0665:5161`, commands are sent as zero-padded HID output reports. Commands
shorter than 8 bytes use an 8-byte report; commands whose payload plus `\r` is
longer use that exact length (for example, `S.5R0000\r` is one 9-byte report).

| Field          | Value                              |
|----------------|------------------------------------|
| bmRequestType  | `0x21` (OUT, Class, Interface)     |
| bRequest       | `0x09` (SET_REPORT)                |
| wValue         | `0x0200` (Output report, ID 0)     |
| wIndex         | `0x0000` (interface 0)             |

If the device rejects the output report, the driver retries the same payload as
a feature report (`wValue = 0x0300`), matching the official app's fallback path.

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

Cypress `V` responses are ASCII. Cypress `T` `QS` responses are compact binary
frames decoded according to the official GCUPS app: bytes are rendered as hex
fields, spaces delimit fields, and `0x28` escapes control bytes.

### Command acknowledgement

MEC0003 descriptor commands return the string `UPS No Ack` on success. Any
other response indicates the command was not understood.

Cypress acknowledgement depends on the QS sub-protocol, which the driver detects
from the first `QS` reply (a leading `(` means `V`, otherwise `T`) or by querying
`M`, then caches:

- **`V`**: the device replies `ACK\r` on success. The driver reads the reply and
  accepts no response (within the ack timeout), `ACK`, or `(ACK` as success; any
  other payload is a negative acknowledgement.
- **`T`**: commands are fire-and-forget. The official app never reads a reply for
  `T`, so the driver writes the command and reports success without reading.

## Logical reports and commands

The Rust API uses the following logical report IDs. On MEC0003, these are USB
string descriptor indices. On Cypress HID, supported entries map to GreenCell QS
commands.

### Queries

| Mnemonic | Logical ID | Cypress command | Description              |
|----------|------------|-----------------|--------------------------|
| M        | `0x01`     | `M\r`           | Cypress T/V protocol ID  |
| M        | `0x02`     | `M\r`           | Cypress T/V protocol ID  |
| QS       | `0x03`     | `QS\r`          | Current (live) parameters |
| M        | `0x0c`     | `M\r`           | Cypress T/V protocol ID  |
| F        | `0x0d`     | `F\r`           | Nominal parameters for Cypress V only |

### Commands

| Mnemonic | Logical ID | Cypress command | Description                   |
|----------|------------|-----------------|-------------------------------|
| T        | `0x04`     | `T\r`           | Start short self-test (~10 s) |
| TL       | `0x05`     | `T\r`           | Start long self-test request  |
| Q        | `0x07`     | `Q\r`           | Toggle beeper on/off          |
| S        | `0x08`     | generated       | Shutdown                      |
| C        | `0x0a`     | `C\r`           | Cancel shutdown / wake up     |
| CT       | `0x0b`     | `C\r`           | Cancel self-test              |
| SR       | `0x10`     | generated       | Shutdown with restore         |
| CSR      | `0x1a`     | `C\r`           | Cancel shutdown-and-restore   |
| CS       | `0x2a`     | `C\r`           | Cancel shutdown-return        |
| *        | see table  | generated       | Timed shutdown variants       |

## Nominal parameters

MEC0003 and Cypress `V` return rated specifications from report/command `F`.
Cypress `T` has no separate `F` nominal report in the official app; nominal
input voltage, battery voltage, and frequency are derived from field `P` in the
compact `QS` frame.

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

### Cypress `T` nominal bitfield

For Cypress `T`, the final compact-frame byte `P` encodes the nominal values:

| Bits | Meaning | Values |
|------|---------|--------|
| 0-2  | nominal input voltage | `0`=110 V, `1`=120 V, `2`=220 V, `3`=230 V, `4`=240 V |
| 5-6  | nominal battery voltage | `0`=12 V, `1`=24 V, `2`=36 V, `3`=48 V |
| 7    | nominal input frequency | `0`=50 Hz, `1`=60 Hz |

The official app does not encode nominal input current for Cypress `T`; this
driver exposes it as `-1.0`.

## Current parameters (`0x03`)

MEC0003 and Cypress `V` return ASCII Q1/QS-style live readings and an 8-bit
status register. Cypress `T` returns the official GCUPS compact QS frame; the
driver decodes it to the same public `UpsStatus` fields.

### ASCII Q1/QS format

MEC0003 `Q1` and Cypress `V` `QS` share the same ASCII shape:

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

### Cypress `T` compact QS format

The official GCUPS app decodes Cypress `T` `QS` replies as a stream of bytes.
The decoder:

1. reads 8-byte interrupt packets until `\r`;
2. drops byte `0x28` unless it follows another `0x28`;
3. renders spaces (`0x20`), a leading `#`, and `\r` literally;
4. renders all other bytes as two lowercase hexadecimal digits;
5. maps escaped bytes after `0x28`: `0` -> `0d`, `1` -> `11`, `2` -> `13`,
   `3` -> `0a`, `4` -> `20`.

Example from GCUPS 1.1.11:

```
#7501 6c 0001 6c 00 600b 12c000 e6 1e 0b 03\r
```

It parses as:

```
#AB C DE F G HI JKL M N O P\r
```

| Field | Width | Formula / meaning |
|-------|-------|-------------------|
| `AB`  | 2 bytes | input voltage numerator |
| `C`   | 1 byte  | input voltage multiplier |
| `DE`  | 2 bytes | output voltage numerator |
| `F`   | 1 byte  | output voltage multiplier |
| `G`   | 1 byte  | load percentage |
| `HI`  | 2 bytes | frequency divisor |
| `JKL` | 3 bytes | frequency numerator |
| `M`   | 1 byte  | battery voltage numerator |
| `N`   | 1 byte  | battery voltage multiplier |
| `O`   | 1 byte  | status register |
| `P`   | 1 byte  | nominal bitfield |

Formulas:

```
input_voltage  = AB * C / 51 / 256
output_voltage = DE * F / 51 / 256
load_percent   = G
frequency      = JKL / HI
battery_voltage = M * N / 510
register       = O
nominal        = decode(P)
```

`input_voltage_fault` and `temperature` are not encoded in Cypress `T`; the
driver exposes them as `-1.0` and `None` respectively.

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

## Device info

MEC0003 report `I` (`0x0c`) returns a padded model string, e.g.:

```
#                2000VA
```

Trim whitespace and the leading `#` to extract the model designation.

Cypress maps the logical info report to command `M\r` in the official app, but
that command returns only the protocol subtype (`T` or `V`), not a model string.
The Rust public `device_info()` therefore treats Cypress model information as
unsupported; CLI `status` falls back to `unknown`. Model resolution for Cypress
devices would have to be inferred from nominal battery voltage, nominal current
(when known), and topology against the official known-device table.

## Shutdown delay mapping

Shutdown delay encoding is transport-specific. The Rust API selects the greatest
supported delay that does not exceed the requested duration.

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

### Cypress shutdown commands

Green Cell's official Cypress QS driver hard-codes shutdown to:

```
S.5R0000\r
```

That is a 30-second stay-off command. The public Rust API keeps the existing
transport-specific delay selection and emits standard Megatec-style `S` forms:

The generated `<n>` token uses the Megatec delay grid:

| Delay range | `<n>` value                     |
|-------------|---------------------------------|
| 12-54 s     | `.2`-`.9` (tenths of a minute)  |
| 1-10 min    | `01`-`10` (whole minutes)       |

- `shutdown_and_restore(delay)` emits the bare `S<n>\r` form. Per the Megatec
  spec the UPS turns its output off after `<n>`, then reconnects ~10 s after
  utility power is recovered - i.e. it restores on mains return.
- `shutdown(delay)` emits the `S<n>R0000\r` form. `R0000` ("never restore") is
  the convention used by NUT's `blazer`/`nutdrv_qx` drivers for
  `shutdown.stayoff`.

Only the default 30-second stay-off command (`S.5R0000\r`) is confirmed by the
official GCUPS Cypress source. Other Cypress timed shutdown values are inferred
from standard Megatec behavior and should be treated as best-effort until tested
on hardware.

### Prolific serial shutdown commands

Prolific (`067b:2303`) has no delay grid; it encodes the requested duration as
whole minutes (`requested.as_secs() / 60`), matching the official app's `S<v>`
mapping:

- `shutdown(delay)` emits `S<minutes>\r`.
- `shutdown_and_restore(delay)` emits `S<minutes>R<minutes>\r`.

The official Prolific driver leaves cancel-restore (`CSR`) and cancel-return
(`CS`) empty; this driver maps both to the plain `C` cancel so the operation
takes effect.

## Known device models

The official app resolves models from USB ID, nominal battery voltage, nominal
current when available, and topology.

MEC0003 / descriptor devices in the official known-device table all use USB ID
`0001:0000`:

| Model code(s) | Name | Voltage | Current | Battery | Topology | Output | Power | Capacity |
|---------------|------|---------|---------|---------|----------|--------|-------|----------|
| UPS01, UPS06 | PowerProof/AiO | 12 V | 2 A | 1x 12 V | line-interactive | simulated sine | 360 W | 600 VA |
| UPS02, UPS07 | PowerProof/AiO | 12 V | 3 A | 1x 12 V | line-interactive | simulated sine | 480 W | 800 VA |
| UPS03 | PowerProof | 24 V | 4 A | 2x 12 V | line-interactive | simulated sine | 600 W | 1000 VA |
| UPS04 | PowerProof | 24 V | 6 A | 2x 12 V | line-interactive | simulated sine | 900 W | 1500 VA |
| UPS05 | PowerProof | 24 V | 8 A | 2x 12 V | line-interactive | simulated sine | 1200 W | 2000 VA |
| UPS08 | PureWave | 24 V | 4 A | 2x 12 V | line-interactive | pure sine | 700 W | 1000 VA |
| UPS09 | PureWave | 24 V | 8 A | 2x 12 V | line-interactive | pure sine | 1400 W | 2000 VA |
| UPS10 | UPS Online MPII | 24 V | 4 A | 2x 12 V | online | pure sine | 1400 W | 1000 VA |
| UPS13 | RACK TOWER 1KVA | 24 V | 5 A | 2x 12 V | online | pure sine | 900 W | 1000 VA |
| UPS14 | RACK TOWER 2KVA | 48 V | 10 A | 4x 12 V | online | pure sine | 1800 W | 2000 VA |
| UPS15 | RACK TOWER 3KVA | 72 V | 13 A | 6x 12 V | online | pure sine | 2700 W | 3000 VA |

The official app also registers USB ID `09d6:0001` with the same
`Mec003Driver`, but its known-device table does not attach model codes to that
ID. UPS17 is not MEC0003 in the official app; it is `067b:2303` Prolific serial.

Cypress QS devices in the official app:

| Model code(s) | Name | Protocol | Voltage | Current | Battery | Output |
|---------------|------|----------|---------|---------|---------|--------|
| UPSLM360, UPSLM480 | PowerProof LM | T | 12 V | unknown | 1x 12 V | simulated sine |
| UPSLM480 | PowerProof LM | T | 12 V | 3 A | 1x 12 V | simulated sine |
| UPSLM600, UPSLM900, UPSLM1200 | PowerProof LM | T | 24 V | unknown | 2x 12 V | simulated sine |
| UPSLM900 | PowerProof LM | T | 24 V | 6 A | 2x 12 V | simulated sine |
| UPSLM1200 | PowerProof LM | T | 24 V | 8 A | 2x 12 V | simulated sine |
| UPSLP480 | PowerProof LP | V | 12 V | 3 A | 1x 12 V | pure sine |
| UPSLP700 | PowerProof LP | V | 24 V | 4 A | 2x 12 V | pure sine |
| UPSLP1050 | PowerProof LP | V | 24 V | 6 A | 2x 12 V | pure sine |
| UPSLP1400 | PowerProof LP | V | 24 V | 8 A | 2x 12 V | pure sine |

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
