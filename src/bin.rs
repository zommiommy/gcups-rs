/// GreenCell UPS CLI monitor.
///
/// Exit codes:
///   0  — mains power present, battery OK
///   1  — on battery (utility fail)
///   2  — battery low
///   3  — UPS fault
///  10  — device not found or communication error
use std::process::ExitCode;

fn main() -> ExitCode {
    let json = std::env::args().any(|a| a == "--json" || a == "-j");

    let ups = match gcups::Ups::open() {
        Ok(u) => u,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(10);
        }
    };

    let status = match ups.status() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(10);
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&status).unwrap());
    } else {
        println!("{status}");
    }

    if status.ups_fault {
        ExitCode::from(3)
    } else if status.battery_low {
        ExitCode::from(2)
    } else if status.utility_fail {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
