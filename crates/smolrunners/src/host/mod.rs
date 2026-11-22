mod agent;
mod boundary;
mod cache;
mod config;
mod environment;
mod marker;
mod smolvm;
mod storage;

use config::Config;
use std::path::PathBuf;
use std::process::ExitCode;

fn value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn subcommand_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn usage() -> ! {
    eprintln!(
        "usage: smolrunners host --state-dir DIR --runtime-dir DIR --launcher PATH [options] <reap|provision|stop|env|list|exec|storage>"
    );
    std::process::exit(2)
}

pub fn run(args: &[String]) -> ExitCode {
    match execute(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("host: {error}");
            ExitCode::FAILURE
        }
    }
}

pub fn doctor(args: &[String]) -> ExitCode {
    match execute_doctor(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("doctor: {error}");
            ExitCode::FAILURE
        }
    }
}

fn execute_doctor(args: &[String]) -> Result<(), String> {
    let mut state: Option<PathBuf> = None;
    let mut runtime: Option<PathBuf> = None;
    let mut json = false;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--state-dir" => state = Some(PathBuf::from(value(args, &mut index, "--state-dir")?)),
            "--runtime-dir" => {
                runtime = Some(PathBuf::from(value(args, &mut index, "--runtime-dir")?))
            }
            "--json" => json = true,
            "--help" | "-h" => {
                eprintln!("usage: smolrunners doctor --state-dir DIR --runtime-dir DIR [--json]");
                std::process::exit(2)
            }
            other => return Err(format!("unknown option {other}")),
        }
        index += 1;
    }
    let config = Config {
        state: state.ok_or("missing --state-dir")?,
        runtime: runtime.ok_or("missing --runtime-dir")?,
        launcher: PathBuf::new(),
        uid: 60000,
        cpu_max: "200000 100000".to_string(),
        memory_max: "2147483648".to_string(),
        pids_max: "512".to_string(),
        net_allow: "10.0.0.0/8".to_string(),
        hold_after_create_ms: 0,
    };
    let report = boundary::doctor(&config);
    if json {
        println!("{}", serde_json::to_string_pretty(&report).map_err(|error| format!("encode report: {error}"))?);
    } else {
        for item in &report.checks {
            println!("{:<20} {:>7}  {}", item.name, format!("{:?}", item.verdict).to_ascii_lowercase(), item.reason.clone().unwrap_or_default());
        }
        println!("eligible: {}", report.eligible);
    }
    Ok(())
}

fn execute(args: &[String]) -> Result<(), String> {
    let mut state: Option<PathBuf> = None;
    let mut runtime: Option<PathBuf> = None;
    let mut launcher: Option<PathBuf> = None;
    let mut uid = 60000u32;
    let mut cpu_max = "200000 100000".to_string();
    let mut memory_max = "2147483648".to_string();
    let mut pids_max = "512".to_string();
    let mut net_allow = "10.0.0.0/8".to_string();
    let mut hold_after_create_ms = 0u64;

    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--state-dir" => state = Some(PathBuf::from(value(args, &mut index, "--state-dir")?)),
            "--runtime-dir" => {
                runtime = Some(PathBuf::from(value(args, &mut index, "--runtime-dir")?))
            }
            "--launcher" => launcher = Some(PathBuf::from(value(args, &mut index, "--launcher")?)),
            "--uid" => uid = value(args, &mut index, "--uid")?.parse().map_err(|_| "bad --uid")?,
            "--cpu-max" => cpu_max = value(args, &mut index, "--cpu-max")?,
            "--memory-max" => memory_max = value(args, &mut index, "--memory-max")?,
            "--pids-max" => pids_max = value(args, &mut index, "--pids-max")?,
            "--net-allow" => net_allow = value(args, &mut index, "--net-allow")?,
            "--hold-after-create-ms" => {
                hold_after_create_ms = value(args, &mut index, "--hold-after-create-ms")?
                    .parse()
                    .map_err(|_| "bad --hold-after-create-ms")?
            }
            "--help" | "-h" => usage(),
            command if !command.starts_with('-') => {
                index += 1;
                return dispatch(
                    &Config {
                        state: state.ok_or("missing --state-dir")?,
                        runtime: runtime.ok_or("missing --runtime-dir")?,
                        launcher: launcher.ok_or("missing --launcher")?,
                        uid,
                        cpu_max,
                        memory_max,
                        pids_max,
                        net_allow,
                        hold_after_create_ms,
                    },
                    command,
                    &args[index..],
                );
            }
            other => return Err(format!("unknown option {other}")),
        }
        index += 1;
    }
    usage()
}

fn dispatch(config: &Config, command: &str, args: &[String]) -> Result<(), String> {
    match command {
        "env" => {
            for (name, value) in environment::sanitized(&config.vm_dir("example"), &config.runtime) {
                println!("{name}={value}");
            }
            Ok(())
        }
        "reap" => {
            agent::prepare_state(config)?;
            for machine in agent::reap(config)? {
                println!("REAPED {machine}");
            }
            Ok(())
        }
        "list" => {
            agent::prepare_state(config)?;
            agent::list(config, args.iter().any(|argument| argument == "--json"))
        }
        "exec" => {
            let mut name: Option<String> = None;
            let mut flags: Vec<&str> = Vec::new();
            let mut index = 0usize;
            while index < args.len() {
                match args[index].as_str() {
                    "--name" => name = Some(subcommand_value(args, &mut index, "--name")?),
                    "--stream" => flags.push("--stream"),
                    "-i" => flags.push("-i"),
                    "--" => break,
                    other => return Err(format!("exec: unknown option {other}")),
                }
                index += 1;
            }
            let command = args.get(index + 1..).unwrap_or_default();
            if command.is_empty() {
                return Err("exec: missing command after --".to_owned());
            }
            let code = agent::exec(config, &name.ok_or("exec: missing --name")?, &flags, command)?;
            std::process::exit(code)
        }
        "storage" => storage::dispatch(config, args),
        "stop" => {
            let mut name: Option<String> = None;
            let mut index = 0usize;
            while index < args.len() {
                match args[index].as_str() {
                    "--name" => name = Some(subcommand_value(args, &mut index, "--name")?),
                    other => return Err(format!("stop: unknown option {other}")),
                }
                index += 1;
            }
            agent::prepare_state(config)?;
            agent::stop(config, &name.ok_or("stop: missing --name")?)
        }
        "provision" => {
            let mut role: Option<String> = None;
            let mut image: Option<String> = None;
            let mut cpus = 2u32;
            let mut memory = 2048u32;
            let mut storage = 20u32;
            let mut overlay = 10u32;
            let mut reservation_id = None;
            let mut execution_id = None;
            let mut jit_file: Option<PathBuf> = None;
            let mut entrypoint = "/opt/smolrunners/bin/run-job".to_owned();
            let mut index = 0usize;
            while index < args.len() {
                match args[index].as_str() {
                    "--name" => role = Some(subcommand_value(args, &mut index, "--name")?),
                    "--image" => image = Some(subcommand_value(args, &mut index, "--image")?),
                    "--cpus" => {
                        cpus = subcommand_value(args, &mut index, "--cpus")?
                            .parse()
                            .map_err(|_| "bad --cpus")?
                    }
                    "--mem" => {
                        memory = subcommand_value(args, &mut index, "--mem")?
                            .parse()
                            .map_err(|_| "bad --mem")?
                    }
                    "--storage" => {
                        storage = subcommand_value(args, &mut index, "--storage")?
                            .parse()
                            .map_err(|_| "bad --storage")?
                    }
                    "--overlay" => {
                        overlay = subcommand_value(args, &mut index, "--overlay")?
                            .parse()
                            .map_err(|_| "bad --overlay")?
                    }
                    "--reservation-id" => {
                        reservation_id = Some(
                            subcommand_value(args, &mut index, "--reservation-id")?
                                .parse()
                                .map_err(|_| "bad --reservation-id")?,
                        )
                    }
                    "--execution-id" => {
                        execution_id = Some(
                            subcommand_value(args, &mut index, "--execution-id")?
                                .parse()
                                .map_err(|_| "bad --execution-id")?,
                        )
                    }
                    "--jit-file" => {
                        jit_file = Some(PathBuf::from(subcommand_value(args, &mut index, "--jit-file")?))
                    }
                    "--entrypoint" => entrypoint = subcommand_value(args, &mut index, "--entrypoint")?,
                    other => return Err(format!("provision: unknown option {other}")),
                }
                index += 1;
            }
            agent::provision(
                config,
                agent::ProvisionRequest {
                    role: role.ok_or("provision: missing --name")?,
                    image: image.ok_or("provision: missing --image")?,
                    cpus,
                    memory_mib: memory,
                    storage_gib: storage,
                    overlay_gib: overlay,
                    reservation_id,
                    execution_id,
                    runner: jit_file.map(|jit_file| agent::RunnerLaunch { jit_file, entrypoint }),
                },
            )
        }
        other => Err(format!("unknown subcommand {other}")),
    }
}
