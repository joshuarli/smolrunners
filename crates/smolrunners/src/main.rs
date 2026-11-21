mod host;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") => ExitCode::SUCCESS,
        Some("host") => host::run(&args[2..]),
        Some("--version" | "-V") => {
            println!("smolrunners {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: smolrunners <serve|host> [options]");
            ExitCode::from(2)
        }
    }
}
