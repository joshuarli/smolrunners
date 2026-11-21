use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") => {
            let rest = &args[2..];
            let result = if rest.iter().any(|argument| argument == "--scenario") {
                smolrunners::coordinator::serve(rest)
            } else {
                smolrunners::api::serve(rest)
            };
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("serve: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("host") if !matches!(args.get(2).map(String::as_str), Some("invite" | "join")) => {
            smolrunners::host::run(&args[2..])
        }
        Some("doctor") => {
            if args[2..].iter().any(|argument| argument == "--state-dir" || argument == "--runtime-dir") {
                smolrunners::host::doctor(&args[2..])
            } else {
                let transport = match smolrunners::cli::transport_from_args(&args) {
                    Ok(transport) => transport,
                    Err(error) => {
                        eprintln!("doctor: {error}");
                        return ExitCode::FAILURE;
                    }
                };
                let output = smolrunners::cli::run(&args, &transport);
                if !output.stderr.is_empty() {
                    eprint!("{}", output.stderr);
                }
                print!("{}", output.stdout);
                ExitCode::from(output.exit)
            }
        }
        Some("--version" | "-V") => {
            println!("smolrunners {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            let transport = match smolrunners::cli::transport_from_args(&args) {
                Ok(transport) => transport,
                Err(error) => {
                    eprintln!("client: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let output = smolrunners::cli::run(&args, &transport);
            if !output.stderr.is_empty() {
                eprint!("{}", output.stderr);
            }
            print!("{}", output.stdout);
            ExitCode::from(output.exit)
        }
    }
}
