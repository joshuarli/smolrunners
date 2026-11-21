use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use smolrunners_core::domain::{
    Arch, Distro, ImageTarget, Profile, ProfileName, ProfileRevision, ReleaseToken, Shape,
};
use smolrunners_core::lifecycle::LifecycleError;
use smolrunners_core::lifecycle::{Effect, EffectResult, EffectSink};
use smolrunners_core::ledger::{
    HostIncarnation, HostKey, ReservationId, ReservationRequest, Tick,
};
use smolrunners_core::proto::Frame;
use smolrunners_core::store::{StoreError, open_connection};
use smolrunners_core::telemetry::{History, IngestContext, Sample};

use super::{
    Config, JobRuntime, RuntimeState, audit, detached_command, host_command, now_ms,
    smolvm_command,
};

pub(crate) struct Sampler {
    pub(crate) child: Child,
    stop: Arc<AtomicBool>,
}

impl Sampler {
    pub(crate) fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn frame_job_id(frame: &Frame) -> Option<&str> {
    match frame {
        Frame::JobAvailable { job_id, .. }
        | Frame::JobAssigned { job_id, .. }
        | Frame::JobStarted { job_id, .. }
        | Frame::JobCompleted { job_id, .. }
        | Frame::VmDelete { job_id, .. } => Some(job_id.as_str()),
        _ => None,
    }
}

fn effect_error(message: impl Into<String>) -> LifecycleError {
    LifecycleError::Store(StoreError::Migration(message.into()))
}

fn safe_role(job_id: &str) -> String {
    let stem: String = job_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem.trim_matches('-');
    let stem = if stem.is_empty() { "job" } else { stem };
    format!("job{}", &stem[..stem.len().min(24)])
}

pub(crate) struct HostSink<'a> {
    config: &'a Config,
    ledger: &'a mut smolrunners_core::ledger::Ledger,
    runtime: &'a mut RuntimeState,
    history: Arc<Mutex<History>>,
    audit_log: &'a Path,
    job: String,
    pub(crate) sampler: Option<Sampler>,
}

impl<'a> HostSink<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        config: &'a Config,
        ledger: &'a mut smolrunners_core::ledger::Ledger,
        runtime: &'a mut RuntimeState,
        history: Arc<Mutex<History>>,
        audit_log: &'a Path,
        job: String,
    ) -> Self {
        Self {
            config,
            ledger,
            runtime,
            history,
            audit_log,
            job,
            sampler: None,
        }
    }

    pub(crate) fn reservation_id(&self) -> Option<u64> {
        self.find_reservation().map(|id| id.get())
    }

    fn find_reservation(&self) -> Option<ReservationId> {
        let sequence = self
            .runtime
            .jobs
            .get(&self.job)?
            .reservation_sequence?;
        self.ledger
            .reservations()
            .find(|reservation| reservation.sequence == sequence)
            .map(|reservation| reservation.id)
    }

    fn revision(&self) -> Result<ProfileRevision, LifecycleError> {
        let shape = Shape::new(self.config.vcpu, self.config.memory_gib)
            .map_err(|error| effect_error(error.to_string()))?;
        let target = ImageTarget::new(Distro::Debian, ReleaseToken::from_parts(13, None), Arch::Arm64);
        let name = ProfileName::new(&self.config.profile)
            .map_err(|error| effect_error(error.to_string()))?;
        Ok(ProfileRevision::new(Profile::new(name, target, shape), 1))
    }
}

impl EffectSink for HostSink<'_> {
    fn perform(&mut self, effect: Effect) -> Result<EffectResult, LifecycleError> {
        let now = Tick::new(now_ms());
        let host_key = HostKey::new(self.config.host_key.clone());
        match effect {
            Effect::ReserveCapacity => {
                let request = ReservationRequest::new(&self.revision()?, 8 * 1024 * 1024 * 1024, None);
                let outcome = self
                    .ledger
                    .reserve(request, now)
                    .map_err(|error| effect_error(error.to_string()))?;
                match outcome {
                    smolrunners_core::ledger::ReservationOutcome::Granted(offer) => {
                        let sequence = self
                            .ledger
                            .reservation(offer.reservation_id)
                            .map(|reservation| reservation.sequence);
                        if let Some(job) = self.runtime.jobs.get_mut(&self.job) {
                            job.reservation_id = Some(offer.reservation_id.get());
                            job.reservation_sequence = sequence;
                        }
                        audit(
                            self.audit_log,
                            &format!(
                                "RESERVE job={} reservation={} host={} disk_bytes={}",
                                self.job,
                                offer.reservation_id.get(),
                                offer.host_key.as_str(),
                                offer.disk_bytes
                            ),
                        );
                    }
                    smolrunners_core::ledger::ReservationOutcome::Pending { status, .. } => {
                        return Err(effect_error(format!("reservation pending: {status:?}")));
                    }
                }
            }
            Effect::AcquireJobs => {
                if let Some(reservation) = self.find_reservation() {
                    self.ledger
                        .mark_busy(reservation, HostIncarnation::new(1))
                        .map_err(|error| effect_error(error.to_string()))?;
                    audit(
                        self.audit_log,
                        &format!("ACQUIRE job={} reservation={}", self.job, reservation.get()),
                    );
                }
            }
            Effect::CreateGuest => {
                let job = self
                    .runtime
                    .jobs
                    .get(&self.job)
                    .cloned()
                    .ok_or_else(|| effect_error("unknown job at create"))?;
                let machine = launch_vm(self.config, &job).map_err(effect_error)?;
                if let Some(entry) = self.runtime.jobs.get_mut(&self.job) {
                    entry.machine = Some(machine.clone());
                }
                audit(self.audit_log, &format!("LAUNCH job={} machine={machine}", self.job));
            }
            Effect::StartSampling => {
                let job = self
                    .runtime
                    .jobs
                    .get(&self.job)
                    .cloned()
                    .ok_or_else(|| effect_error("unknown job at sampling"))?;
                self.sampler = Some(
                    start_sampling(self.config, &job, self.history.clone(), self.audit_log)
                        .map_err(effect_error)?,
                );
                audit(self.audit_log, &format!("SAMPLING job={} attempt={}", self.job, job.execution_attempt_id));
            }
            Effect::DestroyResources => {
                if let Some(machine) = self
                    .runtime
                    .jobs
                    .get(&self.job)
                    .and_then(|job| job.machine.clone())
                {
                    teardown_vm(self.config, &machine, self.audit_log).map_err(effect_error)?;
                    if let Some(entry) = self.runtime.jobs.get_mut(&self.job) {
                        entry.machine = None;
                    }
                }
            }
            Effect::ReleaseCapacity => {
                if let Some(reservation) = self.find_reservation() {
                    let id = reservation.get();
                    self.ledger
                        .release(reservation, &host_key, HostIncarnation::new(1), now)
                        .map_err(|error| effect_error(error.to_string()))?;
                    if let Some(entry) = self.runtime.jobs.get_mut(&self.job) {
                        entry.reservation_id = None;
                        entry.reservation_sequence = None;
                    }
                    audit(self.audit_log, &format!("RELEASE job={} reservation={id}", self.job));
                }
            }
            Effect::FlushDeregister | Effect::DeliverJit | Effect::RegisterRunner | Effect::JoinIdentities => {}
        }
        Ok(EffectResult::Committed)
    }
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn detached_host(config: &Config, role: &str, log: &Path) -> Result<(), String> {
    let host_state = if config.guest_exec.is_some() {
        config.guest_state_dir.join("host")
    } else {
        config.state_dir.join("host")
    };
    let runtime = if config.guest_exec.is_some() {
        "/opt/runtime".to_owned()
    } else {
        config.runtime_dir.display().to_string()
    };
    let launcher = if config.guest_exec.is_some() {
        "/work/smolrunners/tests/vm/bin/smolrunners-launcher".to_owned()
    } else {
        config.launcher.display().to_string()
    };
    let program = if config.guest_exec.is_some() {
        Path::new("/work/smolrunners/tests/vm/bin/smolrunners")
    } else {
        config.agent.as_path()
    };
    let memory_mib = (config.memory_gib * 1024).to_string();
    let arguments = vec![
        "host".to_owned(),
        "--state-dir".to_owned(),
        host_state.display().to_string(),
        "--runtime-dir".to_owned(),
        runtime,
        "--launcher".to_owned(),
        launcher,
        "--uid".to_owned(),
        "60000".to_owned(),
        "provision".to_owned(),
        "--name".to_owned(),
        role.to_owned(),
        "--image".to_owned(),
        config.image.clone(),
        "--cpus".to_owned(),
        config.vcpu.to_string(),
        "--mem".to_owned(),
        memory_mib,
        "--storage".to_owned(),
        "20".to_owned(),
        "--overlay".to_owned(),
        "10".to_owned(),
    ];
    let inner = super::guest_inner(program, &arguments);
    detached_command(config, &inner, log)
        .spawn()
        .map_err(|error| format!("spawn host provision: {error}"))?;
    Ok(())
}

fn launch_vm(config: &Config, job: &JobRuntime) -> Result<String, String> {
    let role = safe_role(&job.job_id);
    let log = config.state_dir.join(format!("host-{role}.log"));
    let _ = fs::remove_file(&log);
    let _ = File::create(&log).map_err(|error| format!("host log: {error}"))?;
    detached_host(config, &role, &log)?;
    let mut machine = None;
    for _ in 0..600 {
        let text = read_log(&log);
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("CLAIM ") {
                machine = Some(name.trim().to_owned());
            }
            if line.starts_with("INELIGIBLE ") || line.starts_with("host:") {
                return Err(format!("host rejected {role}: {line}"));
            }
        }
        if text.lines().any(|line| line.starts_with("READY sr-")) && machine.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    let machine = machine.ok_or_else(|| format!("host never claimed {role}"))?;
    if !read_log(&log).lines().any(|line| line.starts_with("READY sr-")) {
        return Err(format!("host never reached READY for {machine}"));
    }
    stage_fixture(config, &machine)?;
    Ok(machine)
}

fn stage_fixture(config: &Config, machine: &str) -> Result<(), String> {
    if config.guest_exec.is_none() {
        return Ok(());
    }
    let mut mkdir = smolvm_command(config, &["machine", "exec", "--name", machine, "--", "mkdir", "-p", "/opt/smolrunners"]);
    if !mkdir.status().map_err(|error| format!("stage mkdir: {error}"))?.success() {
        return Err("stage mkdir failed".to_owned());
    }
    for (source, destination) in [
        (config.collector.clone(), "/opt/smolrunners/collector"),
        (config.executor.clone(), "/opt/smolrunners/executor"),
        (config.fixture_script.clone(), "/opt/smolrunners/scenario.sh"),
    ] {
        let target = format!("{machine}:{destination}");
        let mut target = smolvm_command(
            config,
            &["machine", "cp", &source.display().to_string(), &target],
        );
        if !target.status().map_err(|error| format!("stage cp: {error}"))?.success() {
            return Err(format!("stage cp {source:?} failed"));
        }
    }
    let mut chmod = smolvm_command(
        config,
        &[
            "machine", "exec", "--name", machine, "--", "chmod", "0755",
            "/opt/smolrunners/collector", "/opt/smolrunners/executor",
        ],
    );
    if !chmod.status().map_err(|error| format!("stage chmod: {error}"))?.success() {
        return Err("stage chmod failed".to_owned());
    }
    Ok(())
}

fn seed_context(config: &Config, attempt: i64, host_id: i64) -> Result<(), String> {
    let db = config.state_dir.join("coordinator.sqlite");
    let connection = open_connection(&db).map_err(|error| format!("open context db: {error}"))?;
    connection
        .execute_batch(&format!(
            "INSERT OR IGNORE INTO github_installation
                (id, installation_id, app_id, account_login, scope, runner_group_id,
                 runner_group_name, created_at, updated_at)
             VALUES (1, 1, 1, 'connected', 'all', 1, 'rg', 'now', 'now');
             INSERT OR IGNORE INTO workflow_run
                (id, installation_id, repository, workflow_name, workflow_run_id, head_branch, created_at)
             VALUES (1, 1, 'o/r', 'wf', 1, 'main', 'now');
             INSERT OR IGNORE INTO workflow_run_attempt (id, workflow_run_id, attempt, created_at)
             VALUES (1, 1, 1, 'now');
             INSERT OR IGNORE INTO job (id, workflow_run_attempt_id, github_job_key, name, created_at)
             VALUES (1, 1, 'connected', 'connected', 'now');
             INSERT OR IGNORE INTO execution_attempt (id, job_id, attempt, state, started_at)
             VALUES ({attempt}, 1, 1, 'busy', 'now');
             INSERT OR IGNORE INTO host
                (id, host_key, incarnation, hostname, arch, total_vcpu, total_memory_gib,
                 total_disk_bytes, capabilities_json, admission_state, created_at)
             VALUES ({host_id}, 'sr-host-a', 1, 'sr-host-a', 'arm64', 2, 4,
                     1000, '{{}}', 'eligible', 'now');"
        ))
        .map_err(|error| format!("seed context: {error}"))
}

pub(crate) fn start_sampling(
    config: &Config,
    job: &JobRuntime,
    history: Arc<Mutex<History>>,
    audit_log: &Path,
) -> Result<Sampler, String> {
    let machine = job
        .machine
        .as_deref()
        .ok_or_else(|| "sampling without a machine".to_owned())?;
    seed_context(config, job.execution_attempt_id, job.host_id)?;
    let mut command = smolvm_command(
        config,
        &[
            "machine", "exec", "--name", machine, "-e", "HOME=/tmp", "--stream", "--",
            "/opt/smolrunners/collector", "--interval-ms", "60", "--batch-lines", "18",
        ],
    );
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("start collector: {error}"))?;
    let stdout = child.stdout.take().ok_or("collector stdout unavailable")?;
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let attempt = job.execution_attempt_id;
    let host_id = job.host_id;
    let audit_log = audit_log.to_path_buf();
    thread::spawn(move || {
        let mut count = 0u64;
        let mut announced = false;
        for line in BufReader::new(stdout).lines() {
            if thread_stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok(line) = line else { break };
            if !line.trim_start().starts_with('{') {
                if !announced {
                    audit(&audit_log, &format!("TELEMETRY_NONJSON attempt={attempt} line={line}"));
                    announced = true;
                }
                continue;
            }
            let Ok(sample): Result<Sample, _> = serde_json::from_str(&line) else {
                if !announced {
                    audit(&audit_log, &format!("TELEMETRY_BADJSON attempt={attempt} line={line}"));
                    announced = true;
                }
                continue;
            };
            let outcome = {
                let mut history = match history.lock() {
                    Ok(history) => history,
                    Err(_) => break,
                };
                history
                    .ingest(
                        sample,
                        IngestContext {
                            host_id: Some(host_id),
                            execution_attempt_id: Some(attempt),
                        },
                    )
                    .and_then(|_| history.flush())
            };
            if let Err(error) = outcome {
                if !announced {
                    audit(&audit_log, &format!("TELEMETRY_INGEST_ERROR attempt={attempt} error={error}"));
                    announced = true;
                }
                continue;
            }
            count += 1;
            if count % 10 == 0 {
                audit(&audit_log, &format!("TELEMETRY attempt={attempt} samples={count}"));
            }
        }
    });
    Ok(Sampler { child, stop })
}

pub(crate) fn run_workload(config: &Config, job: &JobRuntime, audit_log: &Path) -> Result<(), String> {
    let machine = job
        .machine
        .as_deref()
        .ok_or_else(|| "workload without a machine".to_owned())?;
    let log = config.state_dir.join(format!("workload-{}.log", job.job_id));
    let output = File::create(&log).map_err(|error| format!("workload log: {error}"))?;
    let status = smolvm_command(
        config,
        &[
            "machine", "exec", "--name", machine, "-e", "HOME=/tmp",
            "-e", "ACTIONS_RUNNER_HOOK_JOB_STARTED=",
            "-e", "ACTIONS_RUNNER_HOOK_JOB_COMPLETED=",
            "--", "/opt/smolrunners/executor", "/opt/smolrunners/scenario.sh",
        ],
    )
    .stdout(output.try_clone().map_err(|error| format!("clone workload log: {error}"))?)
    .stderr(output)
    .status()
    .map_err(|error| format!("run workload: {error}"))?;
    audit(
        audit_log,
        &format!("WORKLOAD job={} status={}", job.job_id, status),
    );
    if !status.success() {
        return Err(format!("fixture workload failed: {}", read_log(&log)));
    }
    Ok(())
}

fn teardown_vm(config: &Config, machine: &str, audit_log: &Path) -> Result<(), String> {
    let status = host_command(config, &["stop", "--name", machine])
        .status()
        .map_err(|error| format!("host stop: {error}"))?;
    if !status.success() {
        return Err(format!("host stop failed for {machine}"));
    }
    for _ in 0..600 {
        let running = smolvm_command(config, &["machine", "status", "--name", machine])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("machine status: {error}"))?
            .success();
        if !running {
            audit(audit_log, &format!("TEARDOWN machine={machine}"));
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(format!("machine {machine} survived teardown"))
}
