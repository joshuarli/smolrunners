use std::process::ExitCode;

fn main() -> ExitCode {
    let command = std::env::args().nth(1);
    match command.as_deref() {
        Some("serve" | "host") => ExitCode::SUCCESS,
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
