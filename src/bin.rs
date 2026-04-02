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
            power_source: if status.utility_fail { "battery" } else { "mains" },
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
        let topology = if self.offline { "line-interactive" } else { "online (double-conversion)" };
        let temp = self
            .temperature
            .map(|t| format!("{t:.1} C"))
            .unwrap_or_else(|| "n/a".into());

        println!("Device");
        println!("  Model:              {}", self.model);
        println!("  Protocol:           {} v{}", self.protocol, self.protocol_version);
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
        println!("  Input frequency:    {:.1} Hz", self.nominal_input_frequency);
        println!("  Battery voltage:    {:.1} V", self.nominal_battery_voltage);
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
