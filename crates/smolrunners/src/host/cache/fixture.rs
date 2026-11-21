#[path = "mod.rs"]
mod host_cache;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use host_cache::CacheHost;
use host_cache::docker::DockerCache;
use serde_json::{Value, json};

fn now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix-{seconds}")
}

fn value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn number(args: &[String], index: &mut usize, flag: &str) -> Result<u64, String> {
    let text = value(args, index, flag)?;
    text.parse().map_err(|_| format!("bad {flag}: {text}"))
}

fn execute(args: &[String]) -> Result<Value, String> {
    let mut state = String::new();
    let mut repository = String::new();
    let mut tier = String::from("maintainer");
    let mut job = String::new();
    let mut generation = String::new();
    let mut name = String::new();
    let mut path = String::new();
    let mut data = String::new();
    let mut source = String::new();
    let mut target = String::new();
    let mut evidence = String::new();
    let mut output = String::new();
    let mut prior = String::new();
    let mut unpack_ms = 0u64;
    let mut build_ms = 0u64;
    let mut generations = 64u64;
    let mut bytes = 8u64 * 1024 * 1024 * 1024;
    let mut command = String::new();

    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--state" => state = value(args, &mut index, "--state")?,
            "--repo" => repository = value(args, &mut index, "--repo")?,
            "--tier" => tier = value(args, &mut index, "--tier")?,
            "--job" => job = value(args, &mut index, "--job")?,
            "--gen" => generation = value(args, &mut index, "--gen")?,
            "--name" => name = value(args, &mut index, "--name")?,
            "--path" => path = value(args, &mut index, "--path")?,
            "--data" => data = value(args, &mut index, "--data")?,
            "--source" => source = value(args, &mut index, "--source")?,
            "--target" => target = value(args, &mut index, "--target")?,
            "--evidence" => evidence = value(args, &mut index, "--evidence")?,
            "--output" => output = value(args, &mut index, "--output")?,
            "--prior" => prior = value(args, &mut index, "--prior")?,
            "--unpack-ms" => unpack_ms = number(args, &mut index, "--unpack-ms")?,
            "--build-ms" => build_ms = number(args, &mut index, "--build-ms")?,
            "--generations" => generations = number(args, &mut index, "--generations")?,
            "--bytes" => bytes = number(args, &mut index, "--bytes")?,
            other if command.is_empty() && !other.starts_with('-') => command = other.to_owned(),
            other => return Err(format!("unknown argument {other}")),
        }
        index += 1;
    }

    if command.is_empty() {
        return Err("missing command".to_owned());
    }
    if state.is_empty() {
        return Err("missing --state".to_owned());
    }
    if repository.is_empty() {
        repository = "acme/widgets".to_owned();
    }
    let host = CacheHost::new(&state);
    let docker = DockerCache::new(&state);
    let stamp = now();
    match command.as_str() {
        "init" => host.init(),
        "seed" => host.seed(&name, &path, &data),
        "quota" => host.set_quota(generations as usize, bytes),
        "baseline" => host.baseline(&name, &repository, &tier, &stamp),
        "action-cache" => host.action_cache(&job, &repository, &tier, &stamp),
        "acquire" => host.acquire(&job, &generation, &repository, &tier, &stamp),
        "write" => host.write(&job, &path, &data),
        "read" => host.read(&job, &path),
        "shared" => host.shared(&job, &path),
        "release" => host.release(&job, &stamp),
        "publish" => host.publish(&generation, &stamp),
        "interrupt" => host.interrupt(&generation),
        "recover" => host.recover(&repository),
        "current" => host.current(&repository),
        "pin" => host.pin(&generation),
        "unpin" => host.unpin(&generation),
        "terminate" => host.terminate(&generation),
        "gc" => host.gc(),
        "history" => host.history(),
        "inspect" => host.inspect(&generation),
        "docker-stage" => {
            if name.is_empty() || source.is_empty() {
                return Err("docker-stage needs --name and --source".to_owned());
            }
            docker.stage(&name, &PathBuf::from(&source), &repository, &tier, &stamp)
        }
        "docker-publish" => {
            if generation.is_empty() {
                return Err("docker-publish needs --gen".to_owned());
            }
            docker.publish(&generation, &repository, &stamp)
        }
        "docker-transfer" => {
            if job.is_empty()
                || generation.is_empty()
                || target.is_empty()
            {
                return Err("docker-transfer needs --job, --gen and --target".to_owned());
            }
            docker.transfer(
                &job,
                &generation,
                &repository,
                &tier,
                &PathBuf::from(&target),
            )
        }
        "docker-drain" => {
            if job.is_empty() || evidence.is_empty() {
                return Err("docker-drain needs --job and --evidence".to_owned());
            }
            docker.drain(
                &job,
                &PathBuf::from(&evidence),
                &repository,
                &tier,
                &stamp,
            )
        }
        "docker-complete" => {
            if job.is_empty() || output.is_empty() {
                return Err("docker-complete needs --job and --output".to_owned());
            }
            let prior_path = if prior.is_empty() {
                None
            } else {
                Some(PathBuf::from(&prior))
            };
            docker.complete(
                &job,
                unpack_ms,
                build_ms,
                &PathBuf::from(&output),
                prior_path.as_deref(),
                &stamp,
            )
        }
        "docker-history" => docker.history(),
        "docker-verify" => {
            if output.is_empty() || generation.is_empty() {
                return Err("docker-verify needs --output and --gen".to_owned());
            }
            let warm_path = if prior.is_empty() {
                PathBuf::from(&output)
            } else {
                PathBuf::from(&prior)
            };
            docker.verify(&PathBuf::from(&output), &warm_path, &generation)
        }
        other => Err(format!("unknown command {other}")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match execute(&args) {
        Ok(value) => {
            println!("{}", serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned()));
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!("{}", json!({"error": error}));
            ExitCode::FAILURE
        }
    }
}
