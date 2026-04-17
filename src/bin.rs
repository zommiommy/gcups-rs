use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use serde::Serialize;

/// GreenCell UPS monitor and control tool.
///
/// Communicates with a GreenCell MEC0003 UPS over USB HID.
/// Requires root or appropriate udev rules for device access.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Full status report (device info, readings, flags, rated specs).
    Status {
        /// Output as JSON.
        #[arg(short, long)]
        json: bool,
    },

    /// Continuously poll the UPS and emit a time-series of readings.
    ///
    /// Runs until Ctrl-C, or until `--count` samples have been collected,
    /// or until `--duration` of wall-clock time has elapsed. One USB
    /// transaction per tick (nominal specs are cached on entry).
    Watch {
        /// Poll interval. Accepts `500ms`, `1s`, `2.5s`, or a bare number (seconds).
        #[arg(short, long, default_value = "500ms", value_parser = parse_duration_arg)]
        interval: Duration,

        /// Stop after this many samples.
        #[arg(short = 'n', long)]
        count: Option<usize>,

        /// Stop after this total wall-clock duration (same syntax as --interval).
        #[arg(short = 'd', long, value_parser = parse_duration_arg)]
        duration: Option<Duration>,

        /// Output format.
        #[arg(long, value_enum, default_value_t = WatchFormat::Human)]
        format: WatchFormat,

        /// Only emit samples whose status register differs from the previous one.
        #[arg(long)]
        changes_only: bool,
    },

    /// Read nominal (rated) parameters.
    Nominal {
        /// Output as JSON.
        #[arg(short, long)]
        json: bool,
    },

    /// Read device info string (model designation).
    Info,

    /// Read protocol identifier.
    Protocol,

    /// Read protocol version.
    ProtocolVersion,

    /// Read a raw USB string descriptor by index.
    Raw {
        /// Descriptor index (decimal or 0x-prefixed hex).
        #[arg(value_parser = parse_u8)]
        index: u8,
    },

    /// Start a short battery self-test (~10 s).
    TestShort,

    /// Start a long battery self-test (~10 min).
    TestLong,

    /// Cancel a running battery self-test.
    TestCancel,

    /// Toggle the UPS beeper on/off.
    Beeper,

    /// Schedule a UPS shutdown (stays off).
    Shutdown {
        /// Delay in seconds before shutdown.
        #[arg(default_value = "30")]
        delay: u64,
    },

    /// Schedule a UPS shutdown with automatic power restore.
    ShutdownRestore {
        /// Delay in seconds before shutdown.
        #[arg(default_value = "30")]
        delay: u64,
    },

    /// Cancel a pending shutdown.
    CancelShutdown,

    /// Cancel a pending shutdown-and-restore.
    CancelShutdownRestore,

    /// Cancel a pending shutdown-return.
    CancelShutdownReturn,

    /// Wake up / restore power.
    Wakeup,
}

fn parse_u8(s: &str) -> Result<u8, String> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u8::from_str_radix(hex, 16).map_err(|e| format!("invalid hex: {e}"))
    } else {
        s.parse().map_err(|e| format!("invalid number: {e}"))
    }
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum WatchFormat {
    /// Human-readable columnar output with change highlighting.
    Human,
    /// One JSON object per line (newline-delimited JSON).
    Json,
    /// Comma-separated values with a header row.
    Csv,
}

/// Parse a human-friendly duration: `500ms`, `1s`, `2.5s`, `5m`, `1h`, or bare seconds.
fn parse_duration_arg(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".into());
    }
    let split = s
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit() && *c != '.')
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let (num_part, unit) = s.split_at(split);
    let value: f64 = num_part
        .parse()
        .map_err(|e| format!("invalid number {num_part:?}: {e}"))?;
    if !value.is_finite() || value < 0.0 {
        return Err("duration must be finite and non-negative".into());
    }
    let seconds = match unit {
        "" | "s" => value,
        "ms" => value / 1000.0,
        "m" => value * 60.0,
        "h" => value * 3600.0,
        other => return Err(format!("unknown unit {other:?} (expected ms, s, m, h)")),
    };
    Ok(Duration::from_secs_f64(seconds))
}

// ── Full status (combines every query the UPS supports) ─────────────────────

/// Everything we can read from the UPS in one shot.
#[derive(Serialize)]
struct FullStatus {
    // Device identity
    model: String,
    protocol: String,
    protocol_version: String,

    // Nominal (rated) parameters
    nominal_input_voltage: f64,
    nominal_input_current: f64,
    nominal_battery_voltage: f64,
    nominal_input_frequency: f64,
    battery_count: u8,

    // Live electrical readings
    input_voltage: f64,
    input_voltage_fault: f64,
    output_voltage: f64,
    load_percent: f64,
    input_frequency: f64,
    battery_voltage: f64,
    temperature: Option<f64>,

    // Computed
    battery_level: u8,

    // Status flags
    power_source: &'static str,
    utility_fail: bool,
    battery_low: bool,
    ups_fault: bool,
    offline: bool,
    bypass_or_boost: bool,
    beeper_on: bool,
    shutdown_active: bool,
    test_in_progress: bool,
}

impl FullStatus {
    fn gather(ups: &gcups::Ups) -> Result<Self, gcups::Error> {
        let status = ups.status()?;
        let model = ups.device_info().unwrap_or_else(|_| "unknown".into());
        let protocol = ups.protocol().unwrap_or_else(|_| "unknown".into());
        let protocol_version = ups.protocol_version().unwrap_or_else(|_| "unknown".into());

        let battery_count = (status.nominal.battery_voltage / 12.0).round() as u8;

        Ok(Self {
            model,
            protocol,
            protocol_version,
            nominal_input_voltage: status.nominal.input_voltage,
            nominal_input_current: status.nominal.input_current,
            nominal_battery_voltage: status.nominal.battery_voltage,
            nominal_input_frequency: status.nominal.input_frequency,
            battery_count,
            input_voltage: status.input_voltage,
            input_voltage_fault: status.input_voltage_fault,
            output_voltage: status.output_voltage,
            load_percent: status.load_percent,
            input_frequency: status.input_frequency,
            battery_voltage: status.battery_voltage,
            temperature: status.temperature,
            battery_level: status.battery_level,
            power_source: if status.utility_fail {
                "battery"
            } else {
                "mains"
            },
            utility_fail: status.utility_fail,
            battery_low: status.battery_low,
            ups_fault: status.ups_fault,
            offline: status.offline,
            bypass_or_boost: status.bypass_or_boost,
            beeper_on: status.beeper_on,
            shutdown_active: status.shutdown_active,
            test_in_progress: status.test_in_progress,
        })
    }

    fn exit_code(&self) -> ExitCode {
        if self.ups_fault {
            ExitCode::from(3)
        } else if self.battery_low {
            ExitCode::from(2)
        } else if self.utility_fail {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    }

    fn print_human(&self) {
        let yes = |b: bool| if b { "yes" } else { "no" };
        let topology = if self.offline {
            "line-interactive"
        } else {
            "online (double-conversion)"
        };
        let temp = self
            .temperature
            .map(|t| format!("{t:.1} C"))
            .unwrap_or_else(|| "n/a".into());

        println!("Device");
        println!("  Model:              {}", self.model);
        println!(
            "  Protocol:           {} v{}",
            self.protocol, self.protocol_version
        );
        println!("  Topology:           {topology}");
        println!();
        println!("Mains");
        println!("  Input voltage:      {:.1} V", self.input_voltage);
        println!("  Input frequency:    {:.1} Hz", self.input_frequency);
        println!("  Fault voltage:      {:.1} V", self.input_voltage_fault);
        println!();
        println!("Output");
        println!("  Output voltage:     {:.1} V", self.output_voltage);
        println!("  Load:               {:.0}%", self.load_percent);
        println!("  Temperature:        {temp}");
        println!();
        println!("Battery");
        println!("  Level:              {}%", self.battery_level);
        println!("  Voltage:            {:.1} V", self.battery_voltage);
        println!(
            "  Pack:               {}x 12 V ({:.0} V nominal)",
            self.battery_count, self.nominal_battery_voltage
        );
        println!("  Low:                {}", yes(self.battery_low));
        println!();
        println!("Status");
        println!("  Power source:       {}", self.power_source);
        println!("  Utility fail:       {}", yes(self.utility_fail));
        println!("  UPS fault:          {}", yes(self.ups_fault));
        println!("  Bypass/boost:       {}", yes(self.bypass_or_boost));
        println!("  Beeper:             {}", yes(self.beeper_on));
        println!("  Shutdown active:    {}", yes(self.shutdown_active));
        println!("  Test in progress:   {}", yes(self.test_in_progress));
        println!();
        println!("Rated");
        println!("  Input voltage:      {:.1} V", self.nominal_input_voltage);
        println!("  Input current:      {:.0} A", self.nominal_input_current);
        println!(
            "  Input frequency:    {:.1} Hz",
            self.nominal_input_frequency
        );
        println!(
            "  Battery voltage:    {:.1} V",
            self.nominal_battery_voltage
        );
    }
}

// ── Main ────────────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    let cli = Cli::parse();

    let ups = match gcups::Ups::open() {
        Ok(u) => u,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(10);
        }
    };

    // No subcommand: quick one-line status for scripting.
    let Some(command) = cli.command else {
        return match ups.status() {
            Ok(status) => {
                if let Some(json) = std::env::args().find(|a| a == "--json" || a == "-j") {
                    let _ = json;
                    println!("{}", serde_json::to_string_pretty(&status).unwrap());
                } else {
                    println!("{status}");
                }
                status_exit_code(&status)
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(10)
            }
        };
    };

    match run(ups, command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(10)
        }
    }
}

fn run(ups: gcups::Ups, command: Command) -> Result<ExitCode, gcups::Error> {
    match command {
        Command::Status { json } => {
            let full = FullStatus::gather(&ups)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&full).unwrap());
            } else {
                full.print_human();
            }
            Ok(full.exit_code())
        }

        Command::Watch {
            interval,
            count,
            duration,
            format,
            changes_only,
        } => run_watch(&ups, interval, count, duration, format, changes_only),

        Command::Nominal { json } => {
            let params = ups.nominal_params()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&params).unwrap());
            } else {
                println!(
                    "Input: {:.1}V {:.1}Hz  Current: {}A  Battery: {:.1}V",
                    params.input_voltage,
                    params.input_frequency,
                    params.input_current,
                    params.battery_voltage,
                );
            }
            Ok(ExitCode::SUCCESS)
        }

        Command::Info => {
            println!("{}", ups.device_info()?);
            Ok(ExitCode::SUCCESS)
        }

        Command::Protocol => {
            println!("{}", ups.protocol()?);
            Ok(ExitCode::SUCCESS)
        }

        Command::ProtocolVersion => {
            println!("{}", ups.protocol_version()?);
            Ok(ExitCode::SUCCESS)
        }

        Command::Raw { index } => {
            println!("{}", ups.read_descriptor(index)?);
            Ok(ExitCode::SUCCESS)
        }

        Command::TestShort => {
            ups.short_test()?;
            println!("Short self-test started.");
            Ok(ExitCode::SUCCESS)
        }

        Command::TestLong => {
            ups.long_test()?;
            println!("Long self-test started.");
            Ok(ExitCode::SUCCESS)
        }

        Command::TestCancel => {
            ups.cancel_test()?;
            println!("Self-test cancelled.");
            Ok(ExitCode::SUCCESS)
        }

        Command::Beeper => {
            ups.toggle_beeper()?;
            println!("Beeper toggled.");
            Ok(ExitCode::SUCCESS)
        }

        Command::Shutdown { delay } => {
            let sd = ups.shutdown(Duration::from_secs(delay))?;
            println!("Shutdown scheduled in {sd} (stays off).");
            Ok(ExitCode::SUCCESS)
        }

        Command::ShutdownRestore { delay } => {
            let sd = ups.shutdown_and_restore(Duration::from_secs(delay))?;
            println!("Shutdown scheduled in {sd} (will restore on mains return).");
            Ok(ExitCode::SUCCESS)
        }

        Command::CancelShutdown => {
            ups.cancel_shutdown()?;
            println!("Shutdown cancelled.");
            Ok(ExitCode::SUCCESS)
        }

        Command::CancelShutdownRestore => {
            ups.cancel_shutdown_restore()?;
            println!("Shutdown-and-restore cancelled.");
            Ok(ExitCode::SUCCESS)
        }

        Command::CancelShutdownReturn => {
            ups.cancel_shutdown_return()?;
            println!("Shutdown-return cancelled.");
            Ok(ExitCode::SUCCESS)
        }

        Command::Wakeup => {
            ups.wake_up()?;
            println!("Wake-up sent.");
            Ok(ExitCode::SUCCESS)
        }
    }
}

// ── Watch (continuous polling) ─────────────────────────────────────────────

fn run_watch(
    ups: &gcups::Ups,
    interval: Duration,
    count: Option<usize>,
    duration: Option<Duration>,
    format: WatchFormat,
    changes_only: bool,
) -> Result<ExitCode, gcups::Error> {
    use std::io::Write;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    // Nominal specs are the UPS's rated values — read once, reuse forever.
    let nominal = ups.nominal_params()?;

    // Format-specific header.
    match format {
        WatchFormat::Human => print_watch_header_human(),
        WatchFormat::Csv => print_watch_header_csv(),
        WatchFormat::Json => {}
    }

    let start = Instant::now();
    let mut prev_reg: Option<u8> = None;
    let mut polled: usize = 0;

    loop {
        let tick = Instant::now();

        match ups.current_status(&nominal) {
            Ok(status) => {
                polled += 1;
                let reg = register_byte(&status);
                let changed = prev_reg.is_some_and(|p| p != reg);
                let first = prev_reg.is_none();

                if !changes_only || first || changed {
                    let t = tick.duration_since(start).as_secs_f64();
                    let ts = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    match format {
                        WatchFormat::Human => print_watch_row_human(t, &status, reg, prev_reg),
                        WatchFormat::Json => print_watch_row_json(t, ts, &status, reg),
                        WatchFormat::Csv => print_watch_row_csv(t, ts, &status, reg),
                    }
                    // Flush so redirected output streams line-by-line.
                    let _ = std::io::stdout().flush();
                }
                prev_reg = Some(reg);
            }
            Err(e) => eprintln!("warning: sample failed: {e}"),
        }

        if count.is_some_and(|n| polled >= n) {
            break;
        }
        if duration.is_some_and(|d| start.elapsed() >= d) {
            break;
        }

        let elapsed = tick.elapsed();
        if interval > elapsed {
            std::thread::sleep(interval - elapsed);
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// Reconstruct the raw 8-bit status register from the parsed flags.
fn register_byte(s: &gcups::UpsStatus) -> u8 {
    (s.beeper_on as u8)
        | ((s.shutdown_active as u8) << 1)
        | ((s.test_in_progress as u8) << 2)
        | ((s.offline as u8) << 3)
        | ((s.ups_fault as u8) << 4)
        | ((s.bypass_or_boost as u8) << 5)
        | ((s.battery_low as u8) << 6)
        | ((s.utility_fail as u8) << 7)
}

fn print_watch_header_human() {
    println!(
        "# bits (high→low): Uf=utility_fail Bl=battery_low By=bypass_boost Fl=ups_fault \
         Of=offline Ts=test_in_progress Sd=shutdown_active Bp=beeper_on"
    );
    println!(
        "{:>7}  {:>6} {:>6} {:>6} {:>5} {:>5} {:>6} {:>5}  {:<8}",
        "t(s)", "Vin", "Vflt", "Vout", "Load", "Hz", "Vbat", "Temp", "register"
    );
}

fn print_watch_row_human(t: f64, s: &gcups::UpsStatus, reg: u8, prev: Option<u8>) {
    let temp = s
        .temperature
        .map(|t| format!("{t:.1}"))
        .unwrap_or_else(|| "---".into());
    let marker = if prev.is_some_and(|p| p != reg) {
        "*"
    } else {
        " "
    };
    let diff = bit_diff(prev, reg);
    println!(
        "{t:>7.2}{marker} {:>6.1} {:>6.1} {:>6.1} {:>4.0}% {:>5.1} {:>6.2} {temp:>5}  {reg:08b}{diff}",
        s.input_voltage,
        s.input_voltage_fault,
        s.output_voltage,
        s.load_percent,
        s.input_frequency,
        s.battery_voltage,
    );
}

/// Human-readable description of bit changes between two register values.
fn bit_diff(prev: Option<u8>, cur: u8) -> String {
    let Some(prev) = prev else {
        return String::new();
    };
    if prev == cur {
        return String::new();
    }
    const NAMES: [&str; 8] = [
        "beeper_on",
        "shutdown_active",
        "test_in_progress",
        "offline",
        "ups_fault",
        "bypass_boost",
        "battery_low",
        "utility_fail",
    ];
    let mut parts = Vec::new();
    for bit in 0..8u8 {
        let old = (prev >> bit) & 1;
        let new = (cur >> bit) & 1;
        if old != new {
            parts.push(format!("{} {}→{}", NAMES[bit as usize], old, new));
        }
    }
    format!("  [{}]", parts.join(", "))
}

fn print_watch_header_csv() {
    println!(
        "t,ts,register,register_bits,input_voltage,input_voltage_fault,output_voltage,\
         load_percent,input_frequency,battery_voltage,temperature,battery_level,\
         utility_fail,battery_low,bypass_or_boost,ups_fault,offline,\
         test_in_progress,shutdown_active,beeper_on"
    );
}

fn print_watch_row_csv(t: f64, ts: f64, s: &gcups::UpsStatus, reg: u8) {
    let temp = s.temperature.map(|t| format!("{t}")).unwrap_or_default();
    println!(
        "{t:.3},{ts:.3},{reg},{reg:08b},{},{},{},{},{},{},{temp},{},{},{},{},{},{},{},{},{}",
        s.input_voltage,
        s.input_voltage_fault,
        s.output_voltage,
        s.load_percent,
        s.input_frequency,
        s.battery_voltage,
        s.battery_level,
        s.utility_fail,
        s.battery_low,
        s.bypass_or_boost,
        s.ups_fault,
        s.offline,
        s.test_in_progress,
        s.shutdown_active,
        s.beeper_on,
    );
}

fn print_watch_row_json(t: f64, ts: f64, s: &gcups::UpsStatus, reg: u8) {
    #[derive(Serialize)]
    struct Sample {
        t: f64,
        ts: f64,
        register: u8,
        register_bits: String,
        input_voltage: f64,
        input_voltage_fault: f64,
        output_voltage: f64,
        load_percent: f64,
        input_frequency: f64,
        battery_voltage: f64,
        temperature: Option<f64>,
        battery_level: u8,
        utility_fail: bool,
        battery_low: bool,
        bypass_or_boost: bool,
        ups_fault: bool,
        offline: bool,
        test_in_progress: bool,
        shutdown_active: bool,
        beeper_on: bool,
    }
    let sample = Sample {
        t,
        ts,
        register: reg,
        register_bits: format!("{reg:08b}"),
        input_voltage: s.input_voltage,
        input_voltage_fault: s.input_voltage_fault,
        output_voltage: s.output_voltage,
        load_percent: s.load_percent,
        input_frequency: s.input_frequency,
        battery_voltage: s.battery_voltage,
        temperature: s.temperature,
        battery_level: s.battery_level,
        utility_fail: s.utility_fail,
        battery_low: s.battery_low,
        bypass_or_boost: s.bypass_or_boost,
        ups_fault: s.ups_fault,
        offline: s.offline,
        test_in_progress: s.test_in_progress,
        shutdown_active: s.shutdown_active,
        beeper_on: s.beeper_on,
    };
    println!("{}", serde_json::to_string(&sample).unwrap());
}

fn status_exit_code(s: &gcups::UpsStatus) -> ExitCode {
    if s.ups_fault {
        ExitCode::from(3)
    } else if s.battery_low {
        ExitCode::from(2)
    } else if s.utility_fail {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_plain_number_is_seconds() {
        assert_eq!(parse_duration_arg("30").unwrap(), Duration::from_secs(30));
        assert_eq!(
            parse_duration_arg("0.5").unwrap(),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn duration_with_units() {
        assert_eq!(
            parse_duration_arg("500ms").unwrap(),
            Duration::from_millis(500)
        );
        assert_eq!(parse_duration_arg("1s").unwrap(), Duration::from_secs(1));
        assert_eq!(
            parse_duration_arg("2.5s").unwrap(),
            Duration::from_millis(2500)
        );
        assert_eq!(parse_duration_arg("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration_arg("1h").unwrap(), Duration::from_secs(3600));
    }

    #[test]
    fn duration_trims_whitespace() {
        assert_eq!(
            parse_duration_arg("  250ms ").unwrap(),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn duration_rejects_bad_input() {
        assert!(parse_duration_arg("").is_err());
        assert!(parse_duration_arg("abc").is_err());
        assert!(parse_duration_arg("10x").is_err());
        assert!(parse_duration_arg("-1s").is_err());
        assert!(parse_duration_arg("inf").is_err());
    }

    #[test]
    fn register_byte_round_trips_all_flags() {
        let nominal = gcups::NominalParams {
            input_voltage: 230.0,
            input_current: 8.0,
            battery_voltage: 24.0,
            input_frequency: 50.0,
        };
        // Use parse_current indirectly via the library, but here we verify our
        // reconstruction matches what the parser ingested. We do this by
        // crafting a known register and asserting every bit round-trips.
        for reg in 0u8..=255u8 {
            let s = gcups::UpsStatus {
                input_voltage: 0.0,
                input_voltage_fault: 0.0,
                output_voltage: 0.0,
                load_percent: 0.0,
                input_frequency: 0.0,
                battery_voltage: 0.0,
                temperature: None,
                battery_level: 0,
                nominal: nominal.clone(),
                beeper_on: reg & 1 == 1,
                shutdown_active: (reg >> 1) & 1 == 1,
                test_in_progress: (reg >> 2) & 1 == 1,
                offline: (reg >> 3) & 1 == 1,
                ups_fault: (reg >> 4) & 1 == 1,
                bypass_or_boost: (reg >> 5) & 1 == 1,
                battery_low: (reg >> 6) & 1 == 1,
                utility_fail: (reg >> 7) & 1 == 1,
            };
            assert_eq!(register_byte(&s), reg, "mismatch at reg=0b{reg:08b}");
        }
    }

    #[test]
    fn bit_diff_empty_when_unchanged() {
        assert_eq!(bit_diff(None, 0b00001000), "");
        assert_eq!(bit_diff(Some(0b00001000), 0b00001000), "");
    }

    #[test]
    fn bit_diff_names_flipped_bits() {
        // utility_fail 0→1, shutdown_active 0→1
        let d = bit_diff(Some(0b00001000), 0b10001010);
        assert!(d.contains("utility_fail 0→1"), "got: {d}");
        assert!(d.contains("shutdown_active 0→1"), "got: {d}");
    }
}
