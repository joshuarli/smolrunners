mod host;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") => match smolrunners::api::serve(&args[2..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("serve: {error}");
                ExitCode::FAILURE
            }
        },
        Some("host") if !matches!(args.get(2).map(String::as_str), Some("invite" | "join")) => {
            host::run(&args[2..])
        }
        Some("--version" | "-V") => {
            println!("smolrunners {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            let transport = smolrunners::cli::HttpTransport::default();
            let output = smolrunners::cli::run(&args, &transport);
            if !output.stderr.is_empty() {
                eprint!("{}", output.stderr);
            }
            print!("{}", output.stdout);
            ExitCode::from(output.exit)
        }
    }
}
