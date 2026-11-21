use std::env;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("launcher: {message}");
    std::process::exit(2);
}

fn take(args: &[String], index: &mut usize, flag: &str) -> String {
    *index += 1;
    args.get(*index)
        .cloned()
        .unwrap_or_else(|| fail(format!("{flag} requires a value")))
}

fn write_limit(directory: &Path, name: &str, value: &str) {
    let path = directory.join(name);
    fs::write(&path, value)
        .unwrap_or_else(|error| fail(format!("write {}: {error}", path.display())));
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut uid = 0u32;
    let mut cgroup: Option<String> = None;
    let mut cpu_max: Option<String> = None;
    let mut memory_max: Option<String> = None;
    let mut pids_max: Option<String> = None;
    let mut net_allow = String::new();

    let mut index = 1usize;
    let mut command_index = args.len();
    while index < args.len() {
        match args[index].as_str() {
            "--uid" => {
                uid = take(&args, &mut index, "--uid")
                    .parse()
                    .unwrap_or_else(|_| fail("bad --uid"));
            }
            "--gid" => {
                let _ = take(&args, &mut index, "--gid");
            }
            "--cgroup" => cgroup = Some(take(&args, &mut index, "--cgroup")),
            "--cpu-max" => cpu_max = Some(take(&args, &mut index, "--cpu-max")),
            "--memory-max" => memory_max = Some(take(&args, &mut index, "--memory-max")),
            "--pids-max" => pids_max = Some(take(&args, &mut index, "--pids-max")),
            "--net-allow" => net_allow = take(&args, &mut index, "--net-allow"),
            "--" => {
                command_index = index + 1;
                break;
            }
            other => fail(format!("unknown option {other}")),
        }
        index += 1;
    }

    if command_index >= args.len() {
        fail("missing command after --");
    }
    let cgroup = cgroup.unwrap_or_else(|| fail("missing --cgroup"));
    if !cgroup.starts_with("/sys/fs/cgroup/sr-") {
        fail("cgroup must be under /sys/fs/cgroup/sr-");
    }

    let directory = Path::new(&cgroup);
    fs::create_dir_all(directory)
        .unwrap_or_else(|error| fail(format!("create {}: {error}", cgroup)));
    if let Some(value) = cpu_max {
        write_limit(directory, "cpu.max", &value);
    }
    if let Some(value) = memory_max {
        write_limit(directory, "memory.max", &value);
    }
    if let Some(value) = pids_max {
        write_limit(directory, "pids.max", &value);
    }
    write_limit(directory, "net.allow", &net_allow);
    write_limit(
        directory,
        "cgroup.procs",
        &std::process::id().to_string(),
    );

    let mut command = Command::new(&args[command_index]);
    command.args(&args[command_index + 1..]);
    command.env("SMOLRUNNERS_NET_ALLOW", &net_allow);
    command.uid(uid);
    command.gid(uid);
    let error = command.exec();
    fail(format!("exec: {error}"));
}
