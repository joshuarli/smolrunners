use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use smolrunners_core::proto::{Frame, SCHEMA_VERSION};
use smolrunners_core::telemetry::Sample;

use super::plan::{self, HostEvent, Outcome, RunnerExit};
use super::{Shared, now_ms};
use crate::api::JobRecord;

const JIT_TIMEOUT: Duration = Duration::from_secs(120);
const AGENT_CHECK_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Observed {
    machine: Option<String>,
    ready: bool,
    runner_started: bool,
    exit: Option<RunnerExit>,
    failure: Option<String>,
    removed: bool,
}

struct Sampler {
    child: Child,
    stop: Arc<AtomicBool>,
}

impl Sampler {
    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn spawn(shared: Arc<Shared>, job_id: i64, resume: bool) {
    thread::spawn(move || {
        let observed = drive(&shared, job_id, resume);
        finalize(&shared, job_id, observed);
    });
}

fn job(shared: &Shared, job_id: i64) -> Option<JobRecord> {
    shared.state.lock().ok()?.job(job_id).cloned()
}

fn update(shared: &Shared, job_id: i64, change: impl FnOnce(&mut JobRecord)) {
    match shared.state.lock() {
        Ok(mut state) => {
            if let Err(error) = state.update_job(job_id, change) {
                shared.audit(&format!("STATE_ERROR job={job_id} error={error}"));
            }
        }
        Err(_) => shared.audit(&format!("STATE_ERROR job={job_id} error=lock poisoned")),
    }
}

fn failed(message: impl Into<String>) -> Observed {
    Observed {
        failure: Some(message.into()),
        ..Observed::default()
    }
}

fn request_jit(shared: &Shared, job: &JobRecord) -> Result<(i64, String), String> {
    let runner_name = job.runner_name.clone().ok_or("execution has no runner name")?;
    let name = smolrunners_core::domain::RunnerName::new(runner_name)
        .map_err(|error| format!("runner name: {error}"))?;
    let reply = shared.adapter.command(
        |id| Frame::JitRequest {
            schema_version: SCHEMA_VERSION,
            id,
            runner_name: name,
        },
        JIT_TIMEOUT,
    )?;
    match reply {
        Frame::CommandResult {
            ok: true,
            runner_id: Some(runner_id),
            encoded_jit_config: Some(jit),
            ..
        } => Ok((runner_id.get(), jit)),
        Frame::CommandResult { error, .. } => Err(error.unwrap_or_else(|| "adapter returned no JIT config".to_owned())),
        other => Err(format!("unexpected adapter reply {}", other.kind())),
    }
}

fn write_secret(path: &PathBuf, secret: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(secret.as_bytes())
        .map_err(|error| format!("write {}: {error}", path.display()))
}

fn start_agent(shared: &Shared, job: &JobRecord, jit_file: &PathBuf) -> Result<Child, String> {
    let log_path = shared.host_log(job.execution_attempt_id);
    let log = File::create(&log_path).map_err(|error| format!("create {}: {error}", log_path.display()))?;
    let error_log = log.try_clone().map_err(|error| format!("clone host log: {error}"))?;
    let profile = &shared.config.profile;
    let limits = job.limits.clone().ok_or("execution has no effective limits")?;
    let mut command = shared.host_command(Some(&limits));
    command.args([
        "provision",
        "--name",
        &format!("x{}", job.execution_attempt_id),
        "--image",
        &profile.image,
        "--cpus",
        &limits.guest_vcpu.to_string(),
        "--mem",
        &(limits.guest_memory_gib * 1024).to_string(),
        "--storage",
        &limits.storage_gib.to_string(),
        "--overlay",
        &profile.overlay_gib.to_string(),
        "--execution-id",
        &job.execution_attempt_id.to_string(),
        "--jit-file",
        &jit_file.display().to_string(),
        "--entrypoint",
        &profile.entrypoint,
    ]);
    if let Some(sequence) = job.reservation_sequence {
        command.args(["--reservation-id", &sequence.to_string()]);
    }
    command
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(error_log)
        .process_group(0)
        .spawn()
        .map_err(|error| format!("spawn host agent: {error}"))
}

fn start_sampler(shared: &Arc<Shared>, job: &JobRecord, machine: &str) -> Result<Sampler, String> {
    let host_id = job.host_id.unwrap_or(shared.host_id);
    let attempt = job.execution_attempt_id;
    shared
        .state
        .lock()
        .map_err(|_| "state lock poisoned".to_owned())?
        .register_execution(
            attempt,
            job.runner_name.as_deref().unwrap_or("unnamed"),
            host_id,
            &shared.config.host.key,
            &shared.config.host.arch,
        )?;
    let interval = shared.config.telemetry.interval_ms.to_string();
    let mut command = shared.host_command(None);
    command
        .args(["exec", "--name", machine, "--stream", "--"])
        .arg(&shared.config.telemetry.collector)
        .args(["--interval-ms", &interval])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|error| format!("start collector: {error}"))?;
    let stdout = child.stdout.take().ok_or("collector stdout unavailable")?;
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_shared = shared.clone();
    thread::spawn(move || {
        let mut count = 0u64;
        let mut reported = false;
        for line in BufReader::new(stdout).lines() {
            if thread_stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok(line) = line else { break };
            let Some(start) = line.find('{') else { continue };
            let Ok(sample) = serde_json::from_str::<Sample>(&line[start..]) else { continue };
            let outcome = match thread_shared.state.lock() {
                Ok(mut state) => state.ingest_sample(sample, attempt, Some(host_id)),
                Err(_) => break,
            };
            match outcome {
                Ok(_) => {
                    count += 1;
                    if count == 1 || count % 60 == 0 {
                        thread_shared.audit(&format!("TELEMETRY attempt={attempt} samples={count}"));
                    }
                }
                Err(error) if !reported => {
                    reported = true;
                    thread_shared.audit(&format!("TELEMETRY_ERROR attempt={attempt} error={error}"));
                }
                Err(_) => {}
            }
        }
    });
    Ok(Sampler { child, stop })
}

fn read_new_lines(path: &PathBuf, offset: &mut u64, partial: &mut String) -> Vec<String> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return Vec::new();
    }
    let mut buffer = String::new();
    let Ok(read) = file.read_to_string(&mut buffer) else {
        return Vec::new();
    };
    *offset += read as u64;
    partial.push_str(&buffer);
    let mut lines = Vec::new();
    while let Some(position) = partial.find('\n') {
        let line: String = partial.drain(..=position).collect();
        lines.push(line);
    }
    lines
}

fn apply(shared: &Arc<Shared>, job_id: i64, observed: &mut Observed, event: HostEvent) {
    match event {
        HostEvent::Claim(machine) => {
            shared.audit(&format!("LAUNCH job={job_id} machine={machine}"));
            let recorded = machine.clone();
            update(shared, job_id, move |job| job.machine = Some(recorded));
            observed.machine = Some(machine);
        }
        HostEvent::Ready => observed.ready = true,
        HostEvent::RunnerStarted => {
            if !observed.runner_started {
                shared.audit(&format!("RUNNER_STARTED job={job_id}"));
                let now = now_ms();
                update(shared, job_id, move |job| {
                    if job.state == "provisioning" {
                        job.state = "idle".to_owned();
                        job.idle_since_ms = Some(now);
                    }
                });
            }
            observed.runner_started = true;
        }
        HostEvent::RunnerExit(exit) => {
            shared.audit(&format!("RUNNER_EXIT job={job_id} exit={exit:?}"));
            let text = format!("{exit:?}");
            update(shared, job_id, move |job| job.runner_exit = Some(text));
            observed.exit = Some(exit);
        }
        HostEvent::Failed(message) => {
            shared.audit(&format!("HOST_FAILURE job={job_id} error={message}"));
            if observed.failure.is_none() {
                observed.failure = Some(message);
            }
        }
        HostEvent::Removed => observed.removed = true,
    }
}

fn agent_owns(shared: &Shared, machine: &str) -> Option<bool> {
    let output = shared.host_command(None).args(["list", "--json"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let entries: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let entry = entries
        .as_array()?
        .iter()
        .find(|entry| entry["machine"].as_str() == Some(machine));
    Some(entry.is_some_and(|entry| entry["owner_alive"].as_bool() == Some(true)))
}

fn drive(shared: &Arc<Shared>, job_id: i64, resume: bool) -> Observed {
    let Some(record) = job(shared, job_id) else {
        return failed("execution record vanished");
    };
    let log = shared.host_log(record.execution_attempt_id);
    let mut agent: Option<Child> = None;
    if resume {
        if !log.exists() {
            return failed("the coordinator stopped before the host agent started");
        }
        shared.audit(&format!("REATTACH job={job_id} attempt={}", record.execution_attempt_id));
    } else {
        let (runner_id, jit) = match request_jit(shared, &record) {
            Ok(value) => value,
            Err(error) => return failed(format!("jit_request: {error}")),
        };
        update(shared, job_id, move |job| job.runner_id = Some(runner_id));
        shared.audit(&format!("JIT job={job_id} runner_id={runner_id}"));
        let jit_file = shared.jit_file(record.execution_attempt_id);
        if let Err(error) = write_secret(&jit_file, &jit) {
            return failed(error);
        }
        drop(jit);
        match start_agent(shared, &record, &jit_file) {
            Ok(child) => agent = Some(child),
            Err(error) => {
                let _ = fs::remove_file(&jit_file);
                return failed(error);
            }
        }
    }

    let mut observed = Observed::default();
    let mut sampler: Option<Sampler> = None;
    let mut offset = 0u64;
    let mut partial = String::new();
    let mut last_check = Instant::now();
    loop {
        let lines = read_new_lines(&log, &mut offset, &mut partial);
        let progressed = !lines.is_empty();
        for line in lines {
            if let Some(event) = plan::parse_host_line(&line) {
                apply(shared, job_id, &mut observed, event);
            }
        }
        if observed.removed {
            break;
        }
        if observed.runner_started && observed.exit.is_none() && sampler.is_none() {
            if let (Some(machine), Some(current)) = (observed.machine.clone(), job(shared, job_id)) {
                match start_sampler(shared, &current, &machine) {
                    Ok(started) => {
                        shared.audit(&format!("SAMPLING job={job_id} machine={machine}"));
                        sampler = Some(started);
                    }
                    Err(error) => shared.audit(&format!("SAMPLING_FAILED job={job_id} error={error}")),
                }
            }
        }
        if observed.exit.is_some() {
            if let Some(active) = sampler.take() {
                active.stop();
            }
        }
        if let Some(child) = agent.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                let tail = read_new_lines(&log, &mut offset, &mut partial);
                for line in tail {
                    if let Some(event) = plan::parse_host_line(&line) {
                        apply(shared, job_id, &mut observed, event);
                    }
                }
                if !observed.removed && observed.failure.is_none() {
                    observed.failure = Some(format!("host agent exited {status} before teardown"));
                }
                observed.removed = observed.removed || observed.machine.is_none() || !status.success();
                break;
            }
        } else if !progressed && last_check.elapsed() >= AGENT_CHECK_INTERVAL {
            last_check = Instant::now();
            if let Some(machine) = observed.machine.clone() {
                if agent_owns(shared, &machine) == Some(false) {
                    shared.audit(&format!("AGENT_LOST job={job_id} machine={machine}"));
                    let _ = shared.host_command(None).arg("reap").status();
                    if observed.failure.is_none() && observed.exit.is_none() {
                        observed.failure = Some("host agent died; the VM was reaped".to_owned());
                    }
                    observed.removed = true;
                    break;
                }
            } else {
                observed.failure = Some("host log has no claimed machine".to_owned());
                break;
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    if let Some(active) = sampler.take() {
        active.stop();
    }
    let _ = fs::remove_file(shared.jit_file(record.execution_attempt_id));
    observed
}

fn finalize(shared: &Arc<Shared>, job_id: i64, observed: Observed) {
    let Some(record) = job(shared, job_id) else {
        return;
    };
    if let Some(sequence) = record.reservation_sequence {
        match shared.release(sequence) {
            Ok(()) => shared.audit(&format!("RELEASE job={job_id} reservation_sequence={sequence}")),
            Err(error) => shared.audit(&format!("RELEASE_FAILED job={job_id} error={error}")),
        }
    }
    if let Some(runner_id) = record.runner_id.and_then(|id| smolrunners_core::domain::RunnerId::new(id).ok()) {
        let reply = shared.adapter.command(
            |id| Frame::RemoveRunner {
                schema_version: SCHEMA_VERSION,
                id,
                runner_id,
            },
            Duration::from_secs(60),
        );
        match reply {
            Ok(Frame::CommandResult { ok: true, .. }) => {
                shared.audit(&format!("DEREGISTER job={job_id} runner_id={}", runner_id.get()))
            }
            Ok(Frame::CommandResult { error, .. }) => shared.audit(&format!(
                "DEREGISTER_FAILED job={job_id} error={}",
                error.unwrap_or_default()
            )),
            Ok(other) => shared.audit(&format!("DEREGISTER_FAILED job={job_id} reply={}", other.kind())),
            Err(error) => shared.audit(&format!("DEREGISTER_FAILED job={job_id} error={error}")),
        }
    }
    let (state, result) = plan::final_state(&Outcome {
        cancel_requested: record.cancel_requested,
        github_result: record.github_result.as_deref(),
        runner_exit: observed.exit.as_ref(),
        failure: observed.failure.as_deref(),
    });
    let now = now_ms();
    let final_state = state.to_owned();
    let final_result = result.clone();
    update(shared, job_id, move |job| {
        job.state = final_state;
        job.result = Some(final_result);
        job.finish_at_ms = Some(now);
    });
    shared.audit(&format!(
        "JOB_DONE job={job_id} attempt={} state={state} result={result} removed={}",
        record.execution_attempt_id, observed.removed
    ));
    shared.wake();
}
