# Repository Guidelines

## Project Overview

Rust library and CLI for communicating with GreenCell UPS devices (MEC0003) over USB HID. The UPS protocol was reverse-engineered from the proprietary [gcups](https://github.com/fajfer/gcups) Electron app (v1.1.7). The device abuses standard USB `GET_DESCRIPTOR(STRING)` requests as a command/query transport — reading a string descriptor at a specific index either returns telemetry data or triggers an action on the UPS.

The project has two targets: a reusable library (`gcups`) and a multi-command CLI binary (`gcups`).

## Architecture

```
src/
  lib.rs   — Library: USB transport, protocol parsing, public API, tests
  bin.rs   — CLI: clap subcommands, output formatting, exit codes
```

**There are no other source files.** Everything lives in these two files. The library is self-contained with no internal module hierarchy.

### Data flow

1. `Ups::open()` finds the USB device (VID=`0x0001`, PID=`0x0000`) via `rusb`, detaches the kernel HID driver, and returns a handle.
2. `Ups::read_descriptor(index)` sends a `GET_DESCRIPTOR(STRING, index)` control transfer, receives a UTF-16LE string descriptor, decodes it to ASCII.
3. Query methods (`status()`, `nominal_params()`, `device_info()`) call `read_descriptor` with the appropriate report ID, then parse the ASCII response.
4. Command methods (`short_test()`, `shutdown()`, etc.) call `read_descriptor` and verify the response is `"UPS No Ack"` (the device's success acknowledgement).
5. `UpsStatus` is computed by combining two reports: nominal (F, `0x0d`) and current (Q1, `0x03`), with battery voltage adjustment based on UPS topology and battery level calculation from voltage thresholds.

### Key constants (lib.rs)

| Constant | Value | Purpose |
|---|---|---|
| `VID` / `PID` | `0x0001` / `0x0000` | USB device identification |
| `BM_REQUEST_TYPE` | `0x80` | IN \| Standard \| Device |
| `B_REQUEST` | `0x06` | GET_DESCRIPTOR |
| `DESC_TYPE_STRING` | `0x0300` | String descriptor type in wValue |
| `BUF_SIZE` | 96 | Max descriptor payload |
| `ACK_RESPONSE` | `"UPS No Ack"` | Command success response |
| `BATTERY_V_LOW_FACTOR` | 0.915 | Low threshold multiplier |
| `BATTERY_V_HIGH_FACTOR` | 1.05 | High threshold multiplier |

### Report IDs (instruction opcodes)

Defined in `mod report` inside `lib.rs`. Queries: `PROTOCOL` (`0x01`), `PROTOCOL_VERSION` (`0x02`), `CURRENT_PARAMS` (`0x03`), `INFO` (`0x0c`), `NOMINAL_PARAMS` (`0x0d`). Commands: `SHORT_TEST` (`0x04`), `LONG_TEST` (`0x05`), `BEEPER_TOGGLE` (`0x07`), `CANCEL_SHUTDOWN` (`0x0a`), `CANCEL_TEST` (`0x0b`), `CANCEL_SHUTDOWN_RESTORE` (`0x1a`), `CANCEL_SHUTDOWN_RETURN` (`0x2a`). Shutdown delays use dynamically computed report IDs from `ShutdownDelay::TABLE`.

## Public API (lib.rs)

### Types

- **`Ups`** — Handle to an open device. Owns a `rusb::DeviceHandle<Context>`.
- **`UpsStatus`** — Live readings: 7 electrical fields, `battery_level` (u8), embedded `NominalParams`, 8 status flags. Implements `Display` (one-liner) and `Serialize`.
- **`NominalParams`** — Rated specs: `input_voltage`, `input_current`, `battery_voltage`, `input_frequency`. All `f64`.
- **`ShutdownDelay`** — Quantized delay with `from_duration(Duration)` lookup. 14 supported steps from 30 s to 9 min.
- **`Error`** — `thiserror` enum: `DeviceNotFound`, `Usb(rusb::Error)`, `NotAcknowledged`, `ResponseTooShort`, `Parse`.

### Methods on `Ups`

| Method | Report | Returns |
|---|---|---|
| `open()` | — | `Result<Ups, Error>` |
| `status()` | F + Q1 | `Result<UpsStatus, Error>` |
| `nominal_params()` | F | `Result<NominalParams, Error>` |
| `device_info()` | I | `Result<String, Error>` |
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

Uses `clap` 4 with derive macros. Default subcommand (no args) is `status`.

### Subcommands

`status [--json]`, `nominal [--json]`, `info`, `protocol`, `protocol-version`, `raw <index>`, `test-short`, `test-long`, `test-cancel`, `beeper`, `shutdown [delay]`, `shutdown-restore [delay]`, `cancel-shutdown`, `cancel-shutdown-restore`, `cancel-shutdown-return`, `wakeup`.

### Exit codes (status command only)

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

# Run tests (no hardware needed — all tests are parsing/logic)
nix-shell -p pkg-config libusb1 --run 'cargo test'

# Run against live UPS (requires root or udev rule)
sudo ./target/release/gcups
sudo ./target/release/gcups status --json
```

On Debian/Ubuntu: `sudo apt install libusb-1.0-0-dev pkg-config` instead of `nix-shell`.

## Code Conventions

### Error handling

- Library uses `thiserror` with a single `Error` enum. Every variant carries context (`report_id`, `detail`, `len`).
- `rusb::Error` is wrapped via `#[from]`.
- Parse errors include the report ID and a human-readable detail string with the raw value that failed.
- Commands validate the `"UPS No Ack"` response; any other response is `Error::NotAcknowledged`.
- The binary maps all `Error` variants to stderr output and exit code 10.

### Naming

- Types: `PascalCase` (`UpsStatus`, `NominalParams`, `ShutdownDelay`).
- Methods: `snake_case`, named after the action (`short_test`, `cancel_shutdown_restore`).
- Constants: `SCREAMING_SNAKE_CASE` in the `report` module and at module level.
- Status flags: named after the protocol's semantics (`utility_fail`, `bypass_or_boost`, `offline`).
- The `offline` field specifically documents that it means UPS topology, not power source.

### Patterns

- **All USB I/O goes through `read_descriptor()`** — both queries and commands use the same transport. Commands just check the response string.
- **Parsing is prefix-then-split**: strip the leading character (`#` or `(`), split on whitespace, parse each field with contextual errors.
- **Battery voltage adjustment**: online UPS (bit 3 = 0) divides reported voltage by `ONLINE_PARALLEL_DIVISOR` (2.0) and multiplies by nominal voltage. Offline/line-interactive (bit 3 = 1) uses the raw value.
- **ShutdownDelay lookup**: `from_duration()` iterates a const table ascending, returning the greatest entry ≤ requested duration.
- **Fallible info queries in bin.rs**: `device_info()`, `protocol()`, `protocol_version()` use `unwrap_or_else` with `"unknown"` fallback so a partial failure doesn't prevent status output.

### Serialization

- `NominalParams` and `UpsStatus` derive `Serialize` for JSON output from the library.
- `FullStatus` in bin.rs derives `Serialize` separately with a flat field layout (no nested objects).
- `temperature` is `Option<f64>` — serialized as `null` when the sensor returns `--.-`.

## Testing

All tests are in `lib.rs` under `#[cfg(test)] mod tests`. No external test files.

**8 unit tests + 1 doctest:**

| Test | What it covers |
|---|---|
| `parse_nominal_typical` | Happy-path nominal parsing |
| `parse_nominal_missing_prefix` | Missing `#` prefix → error |
| `parse_current_mains_present` | Full Q1 parse, mains on, line-interactive |
| `parse_current_on_battery` | Q1 parse with utility_fail set |
| `parse_current_online_ups` | Online topology voltage adjustment |
| `battery_level_boundaries` | Clamping at 0% and 100%, midpoint |
| `shutdown_delay_lookup` | Duration quantization, below-minimum clamp |
| `string_descriptor_decode` | Real captured bytes → ASCII |
| doctest (`lib.rs` line 12) | Compile-check for the quick-start example |

Tests do not require hardware — they exercise parsing and computation logic only. Run with `cargo test`.

## Important Files

| Path | Purpose |
|---|---|
| `src/lib.rs` | Library: types, USB transport, parsing, API, tests |
| `src/bin.rs` | CLI: clap commands, formatting, exit codes |
| `Cargo.toml` | Manifest: edition 2024, lib + bin targets, 5 deps |
| `PROTOCOL.md` | Wire protocol documentation (report IDs, formats, register bits, delay table) |
| `README.md` | Usage, build instructions, API reference, permissions |

## Protocol Reference

See `PROTOCOL.md` for the full wire-level specification. Key points for working with the code:

- **Transport**: `GET_DESCRIPTOR(STRING, index)` — `bmRequestType=0x80`, `bRequest=0x06`, `wValue=0x0300|index`, `wIndex=0x00`, 96-byte buffer.
- **Responses**: USB string descriptors (UTF-16LE). Decoded by `decode_string_descriptor()`: skip 2-byte header, take low byte of each UTF-16LE unit, drop nulls.
- **Nominal format**: `#<voltage> <current> <battery_v> <frequency>`
- **Current format**: `(<input_v> <fault_v> <output_v> <load%> <freq> <batt_v> <temp> <8-bit binary register>`
- **Register bits** (0=LSB): beeper, shutdown_active, test_in_progress, offline, ups_fault, bypass_boost, battery_low, utility_fail.
