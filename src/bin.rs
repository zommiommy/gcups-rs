use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

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
    /// Read live UPS status (default when no subcommand given).
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Status { json: false });

    let ups = match gcups::Ups::open() {
        Ok(u) => u,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(10);
        }
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
            let status = ups.status()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status).unwrap());
            } else {
                println!("{status}");
            }
            Ok(status_exit_code(&status))
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
