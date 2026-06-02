# Repository Guidelines

## Project Overview

Rust library and CLI for communicating with GreenCell UPS devices over USB HID or USB serial. Three official transport families are supported: MEC0003 descriptor HID (`0001:0000`, plus the app's extra `09d6:0001`) reverse-engineered from the proprietary [gcups](https://github.com/fajfer/gcups) Electron app, Cypress HID GreenCell QS (`0665:5161`), and Prolific serial Q1 (`067b:2303`, UPS17). MEC0003 abuses standard USB `GET_DESCRIPTOR(STRING)` requests as a command/query transport. Cypress devices use the official GreenCell `M`/`QS` command family over HID output reports and interrupt reads; Prolific devices use Q1 over a 2400 8N1 serial port.

The project has two targets: a reusable library (`gcups`) and a multi-command CLI binary (`gcups`).

## Architecture

```
src/
  lib.rs      — Crate docs, module declarations, public re-exports
  device.rs   — Device selectors, supported VID/PID table, transport enum
  error.rs    — Public Error enum
  parse.rs    — Megatec response parsing and battery-level calculation
  shutdown.rs — Descriptor, Megatec, and Prolific shutdown delay encoders
  status.rs   — NominalParams and UpsStatus public data types
  ups.rs      — Device enumeration/opening and USB transport I/O
  wire.rs     — USB constants, logical report IDs, Cypress command map, decoders
  bin.rs      — CLI: clap subcommands, output formatting, exit codes, CLI helper tests
tests/
  public_api.rs — Integration tests for public API behavior
```

### Data flow

1. `Ups::open()` enumerates supported USB devices via `rusb` and supported serial devices via `serialport`, then auto-opens only when exactly one supported UPS is attached.
2. `Ups::list_devices()` returns visible supported devices with VID/PID, transport, and copy-pasteable selectors. `Ups::open_with_selector(DeviceSelector)` opens a selected `VID:PID`, `VID:PID@BUS:ADDR`, or `VID:PID@PORT`.
3. MEC0003 I/O sends `GET_DESCRIPTOR(STRING, index)`, receives a UTF-16LE string descriptor, and decodes it to ASCII.
4. Cypress HID I/O sends GreenCell QS commands (`M\r`, `QS\r`, `F\r`, `T\r`, etc.) in zero-padded HID output reports and reads replies from interrupt endpoint `0x81`.
5. Prolific serial I/O sends Q1/command strings terminated by `\r` over a 2400 8N1 serial port and reads ASCII replies.
6. Command methods (`short_test()`, `shutdown()`, etc.) dispatch through the active transport and validate that transport's acknowledgement convention.
7. `UpsStatus` is computed from nominal and current data, with battery voltage adjustment based on UPS topology and battery level calculation from voltage thresholds.

### Key constants (wire.rs)

| Constant | Value | Purpose |
|---|---|---|
| `MEC_VID` / `MEC_PID` | `0x0001` / `0x0000` | MEC0003 descriptor device identification |
| `CYPRESS_VID` / `CYPRESS_PID` | `0x0665` / `0x5161` | Cypress HID GreenCell QS device identification |
| `MEC_ALT_VID` / `MEC_ALT_PID` | `0x09d6` / `0x0001` | MEC0003 descriptor alternate device ID |
| `PROLIFIC_VID` / `PROLIFIC_PID` | `0x067b` / `0x2303` | Prolific serial Q1 device identification |
| `BM_REQUEST_TYPE` | `0x80` | MEC IN \| Standard \| Device |
| `B_REQUEST` | `0x06` | MEC GET_DESCRIPTOR |
| `DESC_TYPE_STRING` | `0x0300` | MEC string descriptor type in wValue |
| `CYPRESS_SET_REPORT_REQUEST_TYPE` | `0x21` | Cypress OUT \| Class \| Interface |
| `CYPRESS_SET_REPORT` | `0x09` | Cypress SET_REPORT |
| `CYPRESS_OUTPUT_REPORT` | `0x0200` | Cypress output report, report ID 0 |
| `CYPRESS_FEATURE_REPORT` | `0x0300` | Cypress feature report, report ID 0 (output-report retry fallback) |
| `CYPRESS_INTERRUPT_IN` | `0x81` | Cypress interrupt IN endpoint |
| `BUF_SIZE` | 96 | Max response payload |
| `ACK_RESPONSE` | `"UPS No Ack"` | MEC command success response |
| `BATTERY_V_LOW_FACTOR` | 0.915 | Low threshold multiplier (parse.rs) |
| `BATTERY_V_HIGH_FACTOR` | 1.05 | High threshold multiplier (parse.rs) |

### Logical report IDs / instruction opcodes

Defined in `mod report` inside `wire.rs`. Queries: `PROTOCOL` (`0x01`, Cypress `M\r`), `PROTOCOL_VERSION` (`0x02`, Cypress `M\r`), `CURRENT_PARAMS` (`0x03` / Cypress `QS\r`), `INFO` (`0x0c` / Cypress `M\r`), `NOMINAL_PARAMS` (`0x0d` / Cypress `F\r` for protocol `V`). Commands: `SHORT_TEST` (`0x04` / `T\r`), `LONG_TEST` (`0x05` / Cypress `T\r`), `BEEPER_TOGGLE` (`0x07` / `Q\r`), `SHUTDOWN` (`0x08` / generated shutdown), `CANCEL_SHUTDOWN` (`0x0a` / `C\r`), `CANCEL_TEST` (`0x0b` / Cypress `C\r`), `SHUTDOWN_RESTORE` (`0x10` / generated shutdown-restore), `CANCEL_SHUTDOWN_RESTORE` (`0x1a` / `C\r`), `CANCEL_SHUTDOWN_RETURN` (`0x2a` / `C\r`). MEC shutdown delays use `DescriptorShutdownDelay::TABLE`; Cypress shutdown delays use `MegatecShutdownDelay::TABLE`.

## Public API (lib.rs)

### Types

- **`Ups`** — Handle to an open device. Owns the active USB or serial transport.
- **`DeviceInfo`** — Supported connected device: VID/PID, USB bus/address or serial path, transport, selector.
- **`DeviceSelector`** — User selector parsed from `VID:PID`, `VID:PID@BUS:ADDR`, or `VID:PID@PORT`.
- **`DeviceLocation`** — USB bus/address pair; the optional HID physical-location part of a `DeviceSelector`.
- **`UpsTransport`** — Supported transport enum: descriptor HID, Cypress HID QS, or Prolific serial Q1.
- **`UpsStatus`** — Live readings: 7 electrical fields, `battery_level` (u8), embedded `NominalParams`, 8 status flags. Implements `Display` (one-liner) and `Serialize`.
- **`NominalParams`** — Rated specs: `input_voltage`, `input_current`, `battery_voltage`, `input_frequency`. All `f64`.
- **`ShutdownDelay`** — Transport-specific quantized delay returned by shutdown methods.
- **`Error`** — `thiserror` enum: includes device-not-found/ambiguous selector errors, USB errors, acknowledgement errors, short responses/writes, unsupported reports, and parse errors.

### Methods on `Ups`

| Method | Report | Returns |
|---|---|---|
| `open()` | — | `Result<Ups, Error>`; auto-opens only when exactly one supported UPS is connected |
| `list_devices()` | — | `Result<Vec<DeviceInfo>, Error>` |
| `open_with_selector(DeviceSelector)` | — | `Result<Ups, Error>` |
| `status()` | F+Q1 or QS | `Result<UpsStatus, Error>` |
| `nominal_params()` | F or QS | `Result<NominalParams, Error>` |
| `device_info()` | I or M | `Result<String, Error>` |
| `protocol()` | 0x01 | `Result<String, Error>` |
| `protocol_version()` | 0x02 | `Result<String, Error>` |
| `short_test()` | T | `Result<(), Error>` |
| `long_test()` | TL | `Result<(), Error>` |
| `cancel_test()` | CT | `Result<(), Error>` |
| `toggle_beeper()` | Q | `Result<(), Error>` |
| `shutdown(Duration)` | varies | `Result<ShutdownDelay, Error>` |
| `shutdown_and_restore(Duration)` | varies | `Result<ShutdownDelay, Error>` |
| `cancel_shutdown()` | C | `Result<(), Error>` |
| `cancel_shutdown_restore()` | CSR | `Result<(), Error>` |
| `cancel_shutdown_return()` | CS | `Result<(), Error>` |
| `wake_up()` | C | `Result<(), Error>` |
| `read_descriptor(u8)` | any | `Result<String, Error>` |

## CLI (bin.rs)

Uses `clap` 4 with derive macros. No subcommand prints quick one-line status; `status` prints the full report.

### Subcommands

`list`, `status [--json]`, `nominal [--json]`, `info`, `protocol`, `protocol-version`, `raw <index>`, `watch [-i INTERVAL] [-n COUNT] [-d DURATION] [--format human|json|csv] [--changes-only]`, `test-short`, `test-long`, `test-cancel`, `beeper`, `shutdown [delay]`, `shutdown-restore [delay]`, `cancel-shutdown`, `cancel-shutdown-restore`, `cancel-shutdown-return`, `wakeup`. Global option: `--device VID:PID[@BUS:ADDR|@PORT]`.

### Exit codes (status, watch, and the bare one-liner)

| Code | Meaning |
|---|---|
| 0 | Mains present, battery OK |
| 1 | Utility fail (on battery) |
| 2 | Battery low |
| 3 | UPS fault |
| 10 | Device/communication error |

### FullStatus

`bin.rs` defines a `FullStatus` struct that aggregates all UPS queries into one object for the `status` command. It calls `ups.status()`, `ups.device_info()`, `ups.protocol()`, and `ups.protocol_version()` — tolerating failures on the info/protocol queries with fallback to `"unknown"`. Has both `print_human()` (sectioned multi-line output) and JSON serialization.

## Development Commands

**System dependency:** `libusb-1.0` development headers.

```bash
# Build (release)
nix-shell -p pkg-config libusb1 --run 'cargo build --release'

# Build (debug)
nix-shell -p pkg-config libusb1 --run 'cargo build'

# Run tests (no hardware needed — tests cover parsing, selection, and logic)
nix-shell -p pkg-config libusb1 --run 'cargo test'

# Run against live UPS (requires root or udev rule)
sudo ./target/release/gcups
sudo ./target/release/gcups status --json
```

On Debian/Ubuntu: `sudo apt install libusb-1.0-0-dev pkg-config` instead of `nix-shell`.

## Code Conventions

### Error handling

- Library uses `thiserror` with a single `Error` enum. Variants carry context (`selector`, `count`, `report_id`, `detail`, `len`).
- `rusb::Error` is wrapped via `#[from]`.
- Parse errors include the report ID and a human-readable detail string with the raw value that failed.
- MEC commands validate the `"UPS No Ack"` response; Prolific serial orders treat an empty reply (the 1 s no-answer case) or `"UPS No Ack"` as success. Cypress acknowledgement depends on the detected QS sub-protocol: `V` reads the reply and accepts no response / `ACK` / `(ACK`, while `T` commands are write-only and always succeed.
- Auto-detection returns an ambiguity error when more than one supported UPS is connected; callers should list devices and pass a selector.
- The binary maps all `Error` variants to stderr output and exit code 10.

### Naming

- Types: `PascalCase` (`UpsStatus`, `NominalParams`, `ShutdownDelay`).
- Methods: `snake_case`, named after the action (`short_test`, `cancel_shutdown_restore`).
- Constants: `SCREAMING_SNAKE_CASE` in the `report` module and at module level.
- Status flags: named after the protocol's semantics (`utility_fail`, `bypass_or_boost`, `offline`).
- The `offline` field specifically documents that it means UPS topology, not power source.

### Patterns

- **All USB I/O goes through the active transport path behind `Ups::read_descriptor()` / command helpers** — query parsing is shared across transports.
- **Device selection is explicit when ambiguous**: auto-open is only for one supported UPS; `VID:PID@BUS:ADDR` selects a physical device when multiple UPSes share a VID/PID.
- **Parsing is prefix-then-split**: strip the leading character (`#` or `(`), split on whitespace, parse each field with contextual errors.
- **Battery voltage adjustment**: online UPS (bit 3 = 0) divides reported voltage by `ONLINE_PARALLEL_DIVISOR` (2.0) and multiplies by nominal voltage. Offline/line-interactive (bit 3 = 1) uses the raw value.
- **Shutdown delay lookup**: descriptor and Cypress transports have separate const delay tables; both select the greatest entry ≤ requested duration. Prolific has no table — it encodes the requested duration as whole minutes (`S<minutes>`).
- **Fallible info queries in bin.rs**: `device_info()`, `protocol()`, `protocol_version()` use `unwrap_or_else` with `"unknown"` fallback so a partial failure doesn't prevent status output.

### Serialization

- `NominalParams` and `UpsStatus` derive `Serialize` for JSON output from the library.
- `FullStatus` in bin.rs derives `Serialize` separately with a flat field layout (no nested objects).
- `temperature` is `Option<f64>` — serialized as `null` when the sensor returns `--.-`.

## Testing

Private parser/transport-helper tests live alongside their modules
(`parse.rs`, `wire.rs`, `device.rs`, `ups.rs`, `shutdown.rs`); CLI helper tests
remain in `bin.rs`. Public API tests live in `tests/public_api.rs`.

Library unit tests cover nominal/current parsing, battery level calculation,
descriptor decoding, Cypress ASCII decoding, supported device IDs, private
selector ambiguity, Cypress command mapping, and Cypress shutdown delay
encoding. Integration tests cover public selector parsing, public device
selectors, transport display strings, and public shutdown delay lookup. CLI
tests cover duration parsing, status-register reconstruction, and watch
bit-diff formatting.

Tests do not require hardware — they exercise parsing, selection, and
computation logic only. Run with `cargo test`.

## Important Files

| Path | Purpose |
|---|---|
| `src/lib.rs` | Crate docs, module declarations, public re-exports |
| `src/device.rs` | Device selectors, supported VID/PID table, transport enum |
| `src/error.rs` | Public `Error` enum |
| `src/parse.rs` | Megatec response parsing and battery-level calculation |
| `src/shutdown.rs` | Descriptor, Megatec, and Prolific shutdown delay encoders |
| `src/status.rs` | Public status and nominal-parameter data types |
| `src/ups.rs` | Device enumeration/opening and USB transport I/O |
| `src/wire.rs` | USB constants, logical report IDs, Cypress command map, decoders |
| `src/bin.rs` | CLI: clap commands, formatting, exit codes, CLI helper tests |
| `tests/public_api.rs` | Integration tests for public library API |
| `Cargo.toml` | Manifest: edition 2024, lib + bin targets, 6 deps |
| `PROTOCOL.md` | Wire protocol documentation (transports, report IDs, formats, register bits, delay tables) |
| `README.md` | Usage, build instructions, API reference, permissions |
| `/tmp/gcups-re/gcups.deb` | Local download of the official GCUPS 1.1.11 Debian package used for reverse-engineering (not part of the repo) |
| `/tmp/gcups-re/data/opt/gcups/resources/app.asar` | Electron bundle extracted from the official package (not part of the repo) |
| `/tmp/gcups-re/app/` | Fully extracted official GCUPS application sources used for protocol audit (not part of the repo) |

## Official GCUPS source extraction (local)

The official app sources used for protocol auditing were extracted locally, not
committed to this repository.

### Source locations

- downloaded package: `/tmp/gcups-re/gcups.deb`
- package payload: `/tmp/gcups-re/data/`
- Electron bundle: `/tmp/gcups-re/data/opt/gcups/resources/app.asar`
- extracted JS sources: `/tmp/gcups-re/app/`

### How it was downloaded

1. Read the apt package index from:
   `https://gcups-static.greencell.global/deb/dists/stable/non-free/binary-amd64/Packages`
2. Took the latest package entry:
   `pool/non-free/g/gcups/stable/gcups_1.1.11_amd64.deb`
3. Downloaded it locally to `/tmp/gcups-re/gcups.deb`

### How the sources were extracted

1. Listed the `.deb` members with `ar t gcups.deb`
2. Extracted `data.tar.xz` from the package
3. Unpacked `data.tar.xz` into `/tmp/gcups-re/data/`
4. Located the Electron bundle at
   `/tmp/gcups-re/data/opt/gcups/resources/app.asar`
5. Extracted `app.asar` into `/tmp/gcups-re/app/`

### Commands used

```bash
# inspect package index to find the latest .deb
read https://gcups-static.greencell.global/deb/dists/stable/non-free/binary-amd64/Packages

# download the official package
fetch https://gcups-static.greencell.global/deb/pool/non-free/g/gcups/stable/gcups_1.1.11_amd64.deb \
  -> /tmp/gcups-re/gcups.deb

# unpack the Debian package payload
ar x /tmp/gcups-re/gcups.deb data.tar.xz
tar -xf data.tar.xz -C /tmp/gcups-re/data

# extract the Electron sources
asar extract /tmp/gcups-re/data/opt/gcups/resources/app.asar /tmp/gcups-re/app
```


## Protocol Reference

See `PROTOCOL.md` for the full wire-level specification. Key points for working with the code:

- **MEC transport**: `GET_DESCRIPTOR(STRING, index)` — `bmRequestType=0x80`, `bRequest=0x06`, `wValue=0x0300|index`, `wIndex=0x00`, 96-byte buffer.
- **Cypress transport**: GreenCell QS commands in zero-padded HID output reports — `bmRequestType=0x21`, `bRequest=0x09`, `wValue=0x0200`, `wIndex=0x00`; replies are read from interrupt endpoint `0x81`.
- **MEC responses**: USB string descriptors (UTF-16LE). Decoded by `decode_string_descriptor()`: skip 2-byte header, take low byte of each UTF-16LE unit, drop nulls.
- **Cypress responses**: protocol `V` uses ASCII `QS`/`F`; protocol `T` uses compact binary `QS` frames decoded in `parse_cypress_t_current()`.
- **Nominal format (MEC/Cypress V)**: `#<voltage> <current> <battery_v> <frequency>`
- **Current format**: `(<input_v> <fault_v> <output_v> <load%> <freq> <batt_v> <temp> <8-bit binary register>`
- **Register bits** (0=LSB): beeper, shutdown_active, test_in_progress, offline, ups_fault, bypass_boost, battery_low, utility_fail.
