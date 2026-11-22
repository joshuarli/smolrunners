mod adapter;
mod config;
mod execution;
mod plan;
#[cfg(test)]
mod tests;

use std::fs::{self, OpenOptions};
use std::io::{BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use smolrunners_core::domain::{Arch, Distro, ImageTarget, Profile, ProfileName, ProfileRevision, ReleaseToken};
use smolrunners_core::lifecycle::{Effect, EffectResult, EffectSink, Lifecycle, LifecycleError, Submitted};
use smolrunners_core::ledger::{
    HostAdmission, HostCapacity, HostIncarnation, HostKey, Ledger, ReservationOutcome, ReservationRequest, Tick,
};
use smolrunners_core::proto::{Frame, SCHEMA_VERSION};
use smolrunners_core::store::Store;

use crate::api::{EffectiveLimits, NewExecution, State};
use adapter::AdapterLink;
use config::{AdapterConfig, PilotConfig};

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

struct RecordOnly;

impl EffectSink for RecordOnly {
    fn perform(&mut self, _effect: Effect) -> Result<EffectResult, LifecycleError> {
        Ok(EffectResult::Committed)
    }
}

pub(crate) struct Shared {
    config: PilotConfig,
    state: Arc<Mutex<State>>,
    ledger: Mutex<Ledger>,
    lifecycle: Mutex<Lifecycle>,
    adapter: AdapterLink,
    host_id: i64,
    audit_log: PathBuf,
    assigned_jobs: AtomicI64,
    stopping: Mutex<std::collections::HashSet<i64>>,
    reservation_pending: std::sync::atomic::AtomicBool,
    wake: (Mutex<bool>, Condvar),
}

impl Shared {
    pub(crate) fn audit(&self, line: &str) {
        let stamped = format!("{} {line}\n", now_ms());
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.audit_log) {
            let _ = file.write_all(stamped.as_bytes());
        }
        eprintln!("coordinator: {line}");
    }

    pub(crate) fn host_log(&self, attempt: i64) -> PathBuf {
        self.config.state_dir.join("hosts").join(format!("x{attempt}.log"))
    }

    pub(crate) fn jit_file(&self, attempt: i64) -> PathBuf {
        self.config.state_dir.join("jit").join(format!("x{attempt}.jit"))
    }

    pub(crate) fn host_command(&self, limits: Option<&EffectiveLimits>) -> Command {
        let host = &self.config.host;
        let mut command = Command::new(&host.agent);
        command
            .arg("host")
            .arg("--state-dir")
            .arg(&host.state_dir)
            .arg("--runtime-dir")
            .arg(&host.runtime_dir)
            .arg("--launcher")
            .arg(&host.launcher);
        if let Some(limits) = limits {
            command
                .arg("--cpu-max")
                .arg(&limits.host_cpu_max)
                .arg("--memory-max")
                .arg(limits.host_memory_max_bytes.to_string())
                .arg("--pids-max")
                .arg(limits.host_pids_max.to_string());
        }
        command
    }

    fn revision(&self) -> Result<ProfileRevision, String> {
        let arch = match self.config.host.arch.as_str() {
            "arm64" => Arch::Arm64,
            "x64" => Arch::X64,
            other => return Err(format!("host.arch {other} is not arm64 or x64")),
        };
        let target = ImageTarget::new(Distro::Debian, ReleaseToken::from_parts(13, None), arch);
        let name = ProfileName::new(self.config.profile.name.clone()).map_err(|error| error.to_string())?;
        Ok(ProfileRevision::new(Profile::new(name, target, self.config.shape()?), 1))
    }

    fn reserve(&self) -> Result<Option<u64>, String> {
        let revision = self.revision()?;
        let disk = u64::from(self.config.profile.storage_gib) * 1024 * 1024 * 1024;
        let mut ledger = self.ledger.lock().map_err(|_| "ledger lock poisoned".to_owned())?;
        let outcome = ledger
            .reserve(ReservationRequest::new(&revision, disk, None), Tick::new(now_ms() as u64))
            .map_err(|error| error.to_string())?;
        match outcome {
            ReservationOutcome::Granted(offer) => {
                let sequence = ledger
                    .reservation(offer.reservation_id)
                    .map(|reservation| reservation.sequence)
                    .ok_or("granted reservation is missing")?;
                ledger
                    .mark_busy(offer.reservation_id, HostIncarnation::new(1))
                    .map_err(|error| error.to_string())?;
                Ok(Some(sequence))
            }
            ReservationOutcome::Pending { .. } => Ok(None),
        }
    }

    pub(crate) fn release(&self, sequence: u64) -> Result<(), String> {
        let mut ledger = self.ledger.lock().map_err(|_| "ledger lock poisoned".to_owned())?;
        let Some(id) = ledger
            .reservations()
            .find(|reservation| reservation.sequence == sequence)
            .map(|reservation| reservation.id)
        else {
            return Ok(());
        };
        ledger
            .release(
                id,
                &HostKey::new(self.config.host.key.clone()),
                HostIncarnation::new(1),
                Tick::new(now_ms() as u64),
            )
            .map_err(|error| error.to_string())
    }

    pub(crate) fn wake(&self) {
        let (flag, signal) = &self.wake;
        if let Ok(mut woken) = flag.lock() {
            *woken = true;
            signal.notify_all();
        }
    }

    fn live(&self) -> Vec<crate::api::JobRecord> {
        self.state.lock().map(|state| state.live_jobs()).unwrap_or_default()
    }
}

fn private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("create {}: {error}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("permit {}: {error}", path.display()))
}

fn open(config: PilotConfig) -> Result<Arc<Shared>, String> {
    private_dir(&config.state_dir)?;
    private_dir(&config.state_dir.join("hosts"))?;
    private_dir(&config.state_dir.join("jit"))?;
    for stale in fs::read_dir(config.state_dir.join("jit")).map_err(|error| format!("list jit dir: {error}"))? {
        if let Ok(entry) = stale {
            let _ = fs::remove_file(entry.path());
        }
    }
    let db_path = config.state_dir.join("coordinator.sqlite");
    let store = Store::open(&db_path).map_err(|error| format!("open store: {error}"))?;
    let lifecycle = Lifecycle::from_store(store).map_err(|error| format!("open lifecycle: {error}"))?;
    let mut ledger = Ledger::open_path(&db_path).map_err(|error| format!("open ledger: {error}"))?;
    let host = &config.host;
    ledger
        .register_host(HostCapacity {
            host_key: HostKey::new(host.key.clone()),
            incarnation: HostIncarnation::new(1),
            hostname: host.key.clone(),
            arch: match host.arch.as_str() {
                "x64" => Arch::X64,
                _ => Arch::Arm64,
            },
            total_vcpu: host.vcpu,
            total_memory_gib: host.memory_gib,
            total_disk_bytes: host.disk_gib * 1024 * 1024 * 1024,
            headroom_vcpu: 0,
            headroom_memory_gib: 0,
            headroom_disk_bytes: 0,
            vmm_overhead: config.overhead(),
            admission: HostAdmission::Eligible,
            cache_zone: None,
            last_contact: Tick::new(now_ms() as u64),
        })
        .map_err(|error| format!("register host: {error}"))?;
    let mut state = State::open(&config.state_dir.join("api"))?;
    let host_id = state.ensure_local_host(
        &host.key,
        &host.arch,
        u16::try_from(host.vcpu).unwrap_or(u16::MAX),
        host.memory_gib,
        now_ms(),
    )?;
    let audit_log = config.state_dir.join("coordinator.log");
    Ok(Arc::new(Shared {
        config,
        state: Arc::new(Mutex::new(state)),
        ledger: Mutex::new(ledger),
        lifecycle: Mutex::new(lifecycle),
        adapter: AdapterLink::new(),
        host_id,
        audit_log,
        assigned_jobs: AtomicI64::new(0),
        stopping: Mutex::new(std::collections::HashSet::new()),
        reservation_pending: std::sync::atomic::AtomicBool::new(false),
        wake: (Mutex::new(false), Condvar::new()),
    }))
}

fn scale(shared: &Arc<Shared>) -> Result<(), String> {
    let assigned = shared.assigned_jobs.load(Ordering::Relaxed);
    let live = shared.live().len();
    let wanted = plan::runners_to_start(shared.config.max_concurrency, assigned, live);
    shared.reservation_pending.store(false, Ordering::Relaxed);
    for _ in 0..wanted {
        let Some(sequence) = shared.reserve()? else {
            shared.reservation_pending.store(true, Ordering::Relaxed);
            shared.audit(&format!("RESERVE_PENDING assigned={assigned} live={live}"));
            return Ok(());
        };
        let spec = NewExecution {
            profile: shared.config.profile.name.clone(),
            labels: shared.config.profile.labels.clone(),
            host_id: shared.host_id,
            limits: shared.config.limits()?,
            reservation_sequence: Some(sequence),
            runner_name_prefix: shared.config.runner_prefix(),
        };
        let job = match shared.state.lock() {
            Ok(mut state) => state.create_execution(spec, now_ms()),
            Err(_) => Err("state lock poisoned".to_owned()),
        };
        let job = match job {
            Ok(job) => job,
            Err(error) => {
                let _ = shared.release(sequence);
                return Err(error);
            }
        };
        shared.audit(&format!(
            "RESERVE job={} attempt={} runner={} reservation_sequence={sequence} assigned={assigned}",
            job.id,
            job.execution_attempt_id,
            job.runner_name.as_deref().unwrap_or("")
        ));
        execution::spawn(shared.clone(), job.id, false);
    }
    Ok(())
}

fn handle_event(shared: &Arc<Shared>, frame: &Frame) -> Result<Frame, String> {
    let submitted: Submitted = shared
        .lifecycle
        .lock()
        .map_err(|_| "lifecycle lock poisoned".to_owned())?
        .submit(frame, &format!("t={}", now_ms()), &mut RecordOnly)
        .map_err(|error| format!("record {}: {error}", frame.kind()))?;
    match frame {
        Frame::Capacity { assigned_jobs, .. } => {
            shared.assigned_jobs.store(*assigned_jobs, Ordering::Relaxed);
            scale(shared)?;
        }
        Frame::JobAvailable { job_id, labels, .. } => {
            shared.audit(&format!("DEMAND job={job_id} labels={}", labels.join(",")));
        }
        Frame::JobStarted {
            job_id,
            request_id,
            runner_name: Some(runner_name),
            ..
        } => joined(shared, runner_name.as_str(), |job| {
            job.state = "busy".to_owned();
            job.protocol_job_id = job_id.to_string();
            job.request_id = request_id.get();
            job.start_at_ms = Some(now_ms());
            job.idle_since_ms = None;
        }),
        Frame::JobCompleted {
            job_id,
            result,
            runner_name: Some(runner_name),
            ..
        } => joined(shared, runner_name.as_str(), |job| {
            job.protocol_job_id = job_id.to_string();
            job.github_result = Some(result.clone());
            job.github_completed_at_ms = Some(now_ms());
            if crate::api::is_terminal(&job.state) {
                let (state, recorded) = plan::final_state(&plan::Outcome {
                    cancel_requested: job.cancel_requested,
                    github_result: Some(result.as_str()),
                    runner_exit: None,
                    failure: None,
                });
                job.state = state.to_owned();
                job.result = Some(recorded);
            }
        }),
        _ => {}
    }
    match &submitted.ack {
        Frame::Ack { recorded: true, .. } => Ok(submitted.ack.clone()),
        _ => Err(format!("refusing to acknowledge unrecorded event {}", frame.id())),
    }
}

fn joined(shared: &Shared, runner_name: &str, change: impl FnOnce(&mut crate::api::JobRecord)) {
    let Ok(mut state) = shared.state.lock() else {
        return;
    };
    let Some(id) = state.job_by_runner(runner_name).map(|job| job.id) else {
        drop(state);
        shared.audit(&format!("UNKNOWN_RUNNER runner={runner_name}"));
        return;
    };
    if let Err(error) = state.update_job(id, change) {
        drop(state);
        shared.audit(&format!("STATE_ERROR job={id} error={error}"));
        return;
    }
    drop(state);
    shared.audit(&format!("JOIN runner={runner_name} job={id}"));
}

enum AdapterEnd {
    Stopped(Option<String>),
    Failed(String),
}

fn run_adapter(shared: &Arc<Shared>) -> AdapterEnd {
    let config = &shared.config.adapter;
    let mut command = Command::new(config.program());
    let scenario = match config {
        AdapterConfig::Github { github_config, .. } => {
            command.arg("--github-config").arg(github_config);
            String::new()
        }
        AdapterConfig::Fake { scenario, scenarios_dir, .. } => {
            command.arg("--fake").env("SMOLRUNNERS_SCENARIOS_DIR", scenarios_dir);
            scenario.clone()
        }
    };
    let log = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(shared.config.state_dir.join("adapter.log"))
    {
        Ok(log) => log,
        Err(error) => return AdapterEnd::Failed(format!("open adapter log: {error}")),
    };
    let mut child = match command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(log)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return AdapterEnd::Failed(format!("spawn adapter: {error}")),
    };
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return AdapterEnd::Failed("adapter pipes unavailable".to_owned());
    };
    shared.adapter.attach(stdin);
    let start = Frame::Run {
        schema_version: SCHEMA_VERSION,
        id: format!("run-{}", now_ms()),
        scenario,
    };
    let mut end = match shared.adapter.write(&start) {
        Ok(()) => None,
        Err(error) => Some(AdapterEnd::Failed(format!("start adapter: {error}"))),
    };
    let mut reader = BufReader::new(stdout);
    while end.is_none() {
        let frame = match adapter::read_frame(&mut reader) {
            Ok(frame) => frame,
            Err(error) => {
                end = Some(AdapterEnd::Failed(format!("read adapter frame: {error}")));
                break;
            }
        };
        match frame {
            Frame::CommandResult { .. } => shared.adapter.resolve(frame),
            Frame::Ack { .. } | Frame::JitRequest { .. } | Frame::RemoveRunner { .. } | Frame::Run { .. } => {
                end = Some(AdapterEnd::Failed(format!("adapter sent a {} frame", frame.kind())));
            }
            Frame::Session { ref session_event, ref reason, .. } if session_event == "adapter_stopped" => {
                end = Some(AdapterEnd::Stopped(reason.clone()));
            }
            event => match handle_event(shared, &event) {
                Ok(ack) => {
                    if let Err(error) = shared.adapter.write(&ack) {
                        end = Some(AdapterEnd::Failed(format!("write ack: {error}")));
                    }
                }
                Err(error) => end = Some(AdapterEnd::Failed(error)),
            },
        }
    }
    shared.adapter.detach();
    let _ = child.kill();
    let _ = child.wait();
    end.unwrap_or(AdapterEnd::Failed("adapter loop ended".to_owned()))
}

fn supervise_adapter(shared: &Arc<Shared>) {
    let mut delay = Duration::from_secs(5);
    loop {
        shared.audit("ADAPTER_START");
        let end = run_adapter(shared);
        match (&shared.config.adapter, end) {
            (AdapterConfig::Fake { .. }, AdapterEnd::Stopped(reason)) => {
                shared.audit(&format!("ADAPTER_STOPPED reason={}", reason.unwrap_or_default()));
                return;
            }
            (_, AdapterEnd::Stopped(reason)) => {
                shared.audit(&format!("ADAPTER_STOPPED reason={}", reason.unwrap_or_default()));
            }
            (_, AdapterEnd::Failed(error)) => shared.audit(&format!("ADAPTER_FAILED error={error}")),
        }
        shared.audit(&format!("ADAPTER_RESTART in_ms={}", delay.as_millis()));
        thread::sleep(delay);
        delay = (delay * 2).min(Duration::from_secs(120));
    }
}

fn tick(shared: &Arc<Shared>) {
    let policy = plan::StopPolicy {
        now_ms: now_ms(),
        assigned_jobs: shared.assigned_jobs.load(Ordering::Relaxed),
        idle_timeout_ms: (shared.config.idle_timeout_secs * 1000) as i64,
        completion_grace_ms: (shared.config.completion_grace_secs * 1000) as i64,
    };
    let live = shared.live();
    for (job_id, reason) in plan::stops(&policy, &live) {
        let Some(machine) = live.iter().find(|job| job.id == job_id).and_then(|job| job.machine.clone()) else {
            continue;
        };
        let first = shared
            .stopping
            .lock()
            .map(|mut stopping| stopping.insert(job_id))
            .unwrap_or(true);
        if !first {
            continue;
        }
        let status = shared
            .host_command(None)
            .args(["stop", "--name", &machine])
            .stdout(Stdio::null())
            .status();
        shared.audit(&format!(
            "STOP job={job_id} machine={machine} reason={} ok={}",
            reason.as_str(),
            status.is_ok_and(|status| status.success())
        ));
    }
    if shared.reservation_pending.load(Ordering::Relaxed) {
        if let Err(error) = scale(shared) {
            shared.audit(&format!("SCALE_FAILED error={error}"));
        }
    }
}

fn run_ticks(shared: Arc<Shared>) {
    thread::spawn(move || {
        loop {
            {
                let (flag, signal) = &shared.wake;
                if let Ok(woken) = flag.lock() {
                    if let Ok((mut woken, _)) = signal.wait_timeout(woken, Duration::from_secs(1)) {
                        *woken = false;
                    }
                }
            }
            tick(&shared);
        }
    });
}

fn recover(shared: &Arc<Shared>) {
    for job in shared.live() {
        execution::spawn(shared.clone(), job.id, true);
    }
}

fn parse_args(args: &[String]) -> Result<PathBuf, String> {
    match args {
        [flag, path] if flag == "--pilot" => Ok(PathBuf::from(path)),
        _ => Err("usage: smolrunners serve --pilot CONFIG.toml".to_owned()),
    }
}

pub fn serve(args: &[String]) -> Result<(), String> {
    let config = PilotConfig::load(&parse_args(args)?)?;
    let listen = config.listen.to_string();
    let shared = open(config)?;
    shared.audit(&format!("BOOT host={} listen={listen}", shared.config.host.key));
    let api_state = shared.state.clone();
    let api_listen = listen.clone();
    let api_shared = shared.clone();
    thread::spawn(move || {
        if let Err(error) = crate::api::serve_state(&api_listen, api_state, None) {
            api_shared.audit(&format!("API_FAILED error={error}"));
            std::process::exit(1);
        }
    });
    recover(&shared);
    run_ticks(shared.clone());
    supervise_adapter(&shared);
    loop {
        thread::park();
    }
}
