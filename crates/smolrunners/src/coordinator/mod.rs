mod effects;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use smolrunners_core::lifecycle::{ExecutionState, Lifecycle, SubmitOutcome, Submitted};
use smolrunners_core::ledger::{
    HostAdmission, HostCapacity, HostIncarnation, HostKey, Ledger, Tick, VmmOverhead,
};
use smolrunners_core::proto::{self, Frame, SCHEMA_VERSION};
use smolrunners_core::store::Store;
use smolrunners_core::telemetry::History;

use effects::Sampler;

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) listen: String,
    pub(crate) state_dir: PathBuf,
    pub(crate) scenarios: Vec<String>,
    pub(crate) scenarios_dir: PathBuf,
    pub(crate) adapter: PathBuf,
    pub(crate) image: String,
    pub(crate) runtime_dir: PathBuf,
    pub(crate) launcher: PathBuf,
    pub(crate) agent: PathBuf,
    pub(crate) profile: String,
    pub(crate) vcpu: u16,
    pub(crate) memory_gib: u32,
    pub(crate) host_key: String,
    pub(crate) host_vcpu: u32,
    pub(crate) host_memory_gib: u32,
    pub(crate) guest_exec: Option<PathBuf>,
    pub(crate) guest_state_dir: PathBuf,
    pub(crate) collector: PathBuf,
    pub(crate) executor: PathBuf,
    pub(crate) fixture_script: PathBuf,
    pub(crate) guest_smolvm: PathBuf,
    pub(crate) guest_machine: String,
    pub(crate) guest_home: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct JobRuntime {
    pub(crate) job_id: String,
    pub(crate) protocol_job_id: String,
    pub(crate) request_id: i64,
    pub(crate) execution_attempt_id: i64,
    pub(crate) host_id: i64,
    pub(crate) profile: String,
    pub(crate) machine: Option<String>,
    pub(crate) reservation_id: Option<u64>,
    pub(crate) reservation_sequence: Option<u64>,
    pub(crate) status: String,
    pub(crate) scenario: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct RuntimeState {
    pub(crate) jobs: BTreeMap<String, JobRuntime>,
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn audit(log: &Path, line: &str) {
    let stamped = format!("{} {}\n", now_ms(), line);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log) {
        let _ = file.write_all(stamped.as_bytes());
    }
    eprintln!("coordinator: {line}");
}

fn value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_args(args: &[String]) -> Result<Config, String> {
    let mut listen = "127.0.0.1:8793".to_owned();
    let mut state_dir = PathBuf::from(".work/coordinator-state");
    let mut scenarios = Vec::new();
    let mut scenarios_dir = std::env::var_os("SMOLRUNNERS_SCENARIOS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("adapter/testdata/scenarios"));
    let mut adapter = PathBuf::from("adapter/smolrunners-adapter");
    let mut image = "alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf".to_owned();
    let mut runtime_dir = PathBuf::from("/opt/runtime");
    let mut launcher = PathBuf::from("tests/vm/bin/smolrunners-launcher");
    let mut agent = std::env::current_exe().map_err(|error| format!("current exe: {error}"))?;
    let mut profile = "fixture-1c2g".to_owned();
    let mut vcpu = 1u16;
    let mut memory_gib = 2u32;
    let mut host_key = "sr-host-a".to_owned();
    let mut host_vcpu = 2u32;
    let mut host_memory_gib = 4u32;
    let mut guest_exec: Option<PathBuf> = None;
    let mut guest_state_dir = PathBuf::from("/work/tmp/connected-job-state");
    let mut collector = PathBuf::from("/work/smolrunners/tests/vm/bin/smolrunners-collector");
    let mut executor = PathBuf::from("/work/smolrunners/tests/fixtures/executor");
    let mut fixture_script = PathBuf::from("/work/tmp/connected-job-state/fixture.sh");
    let mut guest_smolvm = PathBuf::from(".work/runtime/darwin-arm64/smolvm");
    let mut guest_machine = "sr-dev".to_owned();
    let mut guest_home = PathBuf::from(".work/linux-host/home");
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--listen" => listen = value(args, &mut index, "--listen")?,
            "--state-dir" => state_dir = PathBuf::from(value(args, &mut index, "--state-dir")?),
            "--scenario" => {
                for name in value(args, &mut index, "--scenario")?.split(',') {
                    if !name.is_empty() {
                        scenarios.push(name.to_owned());
                    }
                }
            }
            "--scenarios-dir" => {
                scenarios_dir = PathBuf::from(value(args, &mut index, "--scenarios-dir")?)
            }
            "--adapter" => adapter = PathBuf::from(value(args, &mut index, "--adapter")?),
            "--image" => image = value(args, &mut index, "--image")?,
            "--runtime-dir" => runtime_dir = PathBuf::from(value(args, &mut index, "--runtime-dir")?),
            "--launcher" => launcher = PathBuf::from(value(args, &mut index, "--launcher")?),
            "--agent" => agent = PathBuf::from(value(args, &mut index, "--agent")?),
            "--profile" => profile = value(args, &mut index, "--profile")?,
            "--vcpu" => vcpu = value(args, &mut index, "--vcpu")?.parse().map_err(|_| "bad --vcpu")?,
            "--memory-gib" => {
                memory_gib = value(args, &mut index, "--memory-gib")?
                    .parse()
                    .map_err(|_| "bad --memory-gib")?
            }
            "--host-key" => host_key = value(args, &mut index, "--host-key")?,
            "--host-vcpu" => {
                host_vcpu = value(args, &mut index, "--host-vcpu")?
                    .parse()
                    .map_err(|_| "bad --host-vcpu")?
            }
            "--host-memory-gib" => {
                host_memory_gib = value(args, &mut index, "--host-memory-gib")?
                    .parse()
                    .map_err(|_| "bad --host-memory-gib")?
            }
            "--guest-exec" => guest_exec = Some(PathBuf::from(value(args, &mut index, "--guest-exec")?)),
            "--guest-state-dir" => {
                guest_state_dir = PathBuf::from(value(args, &mut index, "--guest-state-dir")?)
            }
            "--collector" => collector = PathBuf::from(value(args, &mut index, "--collector")?),
            "--executor" => executor = PathBuf::from(value(args, &mut index, "--executor")?),
            "--fixture-script" => {
                fixture_script = PathBuf::from(value(args, &mut index, "--fixture-script")?)
            }
            "--guest-smolvm" => guest_smolvm = PathBuf::from(value(args, &mut index, "--guest-smolvm")?),
            "--guest-machine" => guest_machine = value(args, &mut index, "--guest-machine")?,
            "--guest-home" => guest_home = PathBuf::from(value(args, &mut index, "--guest-home")?),
            other => return Err(format!("serve: unknown option {other}")),
        }
        index += 1;
    }
    if scenarios.is_empty() {
        return Err("serve: --scenario requires at least one name".to_owned());
    }
    Ok(Config {
        listen,
        state_dir,
        scenarios,
        scenarios_dir,
        adapter,
        image,
        runtime_dir,
        launcher,
        agent,
        profile,
        vcpu,
        memory_gib,
        host_key,
        host_vcpu,
        host_memory_gib,
        guest_exec,
        guest_state_dir,
        collector,
        executor,
        fixture_script,
        guest_smolvm,
        guest_machine,
        guest_home,
    })
}

pub fn recorded_ack(submitted: &Submitted) -> Result<Frame, String> {
    match &submitted.ack {
        Frame::Ack {
            recorded: true, ..
        } => Ok(submitted.ack.clone()),
        Frame::Ack {
            recorded: false, ..
        } => Err("refusing to emit an unrecorded ack".to_owned()),
        other => Err(format!("expected an ack frame, got {}", other.kind())),
    }
}

pub fn emit_ack<W: Write>(writer: &mut W, submitted: &Submitted) -> Result<(), String> {
    let ack = recorded_ack(submitted)?;
    proto::write_frame(writer, &ack).map_err(|error| error.to_string())
}

fn read_frame<R: Read>(reader: &mut R) -> Result<Frame, String> {
    let mut prefix = [0u8; 4];
    reader
        .read_exact(&mut prefix)
        .map_err(|error| format!("read frame prefix: {error}"))?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > proto::MAX_FRAME_BYTES {
        return Err(format!("invalid adapter frame length {length}"));
    }
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("read frame body: {error}"))?;
    let mut framed = (body.len() as u32).to_be_bytes().to_vec();
    framed.extend_from_slice(&body);
    proto::decode(&framed).map_err(|error| error.to_string())
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn guest_shell(config: &Config, envs: &[(&str, String)]) -> String {
    let mut parts = vec![
        "env".to_owned(),
        "-i".to_owned(),
        format!("PATH={}", quote("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")),
        format!("HOME={}", quote(&config.guest_home.display().to_string())),
        format!("TMPDIR={}", quote(&config.guest_home.join("tmp").display().to_string())),
        format!("XDG_CONFIG_HOME={}", quote(&config.guest_home.join(".config").display().to_string())),
        format!("XDG_CACHE_HOME={}", quote(&config.guest_home.join(".cache").display().to_string())),
    ];
    parts.push(quote(&config.guest_smolvm.display().to_string()));
    parts.push("machine".to_owned());
    parts.push("exec".to_owned());
    parts.push("--name".to_owned());
    parts.push(quote(&config.guest_machine));
    parts.push("--timeout".to_owned());
    parts.push("3h".to_owned());
    for (name, value) in envs {
        parts.push("-e".to_owned());
        parts.push(quote(&format!("{name}={value}")));
    }
    parts.push("-i".to_owned());
    parts.push("--stream".to_owned());
    parts.push("--".to_owned());
    parts.push("sh".to_owned());
    parts.push("-eu".to_owned());
    parts.push("-c".to_owned());
    parts.join(" ")
}

fn guest_inner(program: &Path, args: &[String]) -> String {
    let mut parts = vec![quote(&program.display().to_string())];
    parts.extend(args.iter().map(|argument| quote(argument)));
    parts.join(" ")
}

pub(crate) fn runner(
    config: &Config,
    program: &Path,
    args: &[String],
    envs: &[(&str, String)],
) -> Command {
    if let Some(_helper) = &config.guest_exec {
        let inner = guest_inner(program, args);
        let mut command = Command::new("sh");
        command.arg("-c").arg(format!(
            "{} {}",
            guest_shell(config, envs),
            quote(&inner)
        ));
        command
    } else {
        let mut command = Command::new(program);
        for (name, value) in envs {
            command.env(name, value);
        }
        command.args(args);
        command
    }
}

pub(crate) fn detached_command(config: &Config, inner: &str, log: &Path) -> Command {
    let shell = if config.guest_exec.is_some() {
        format!(
            "{} {} > {} 2>&1 < /dev/null &",
            guest_shell(config, &[]),
            quote(inner),
            quote(&log.display().to_string())
        )
    } else {
        format!("{} > {} 2>&1 < /dev/null &", inner, quote(&log.display().to_string()))
    };
    let mut command = Command::new("sh");
    command.arg("-c").arg(shell);
    command
}

pub(crate) fn smolvm_command(config: &Config, args: &[&str]) -> Command {
    let guest_smolvm = config.runtime_dir.join("smolvm");
    let program = if config.guest_exec.is_some() {
        Path::new("/opt/runtime/smolvm")
    } else {
        &guest_smolvm
    };
    let state = if config.guest_exec.is_some() {
        config.guest_state_dir.join("host/smolvm")
    } else {
        config.state_dir.join("host/smolvm")
    };
    let mut all = vec![
        "--error-format".to_owned(),
        "json".to_owned(),
        "--state-dir".to_owned(),
        state.display().to_string(),
    ];
    all.extend(args.iter().map(|value| (*value).to_owned()));
    let envs = [
        ("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_owned()),
        ("HOME", config.guest_state_dir.join("home").display().to_string()),
        ("TMPDIR", config.guest_state_dir.join("tmp").display().to_string()),
        ("SMOLVM_LIB_DIR", config.runtime_dir.join("lib").display().to_string()),
        (
            "SMOLVM_AGENT_ROOTFS",
            config.runtime_dir.join("agent-rootfs").display().to_string(),
        ),
    ];
    runner(config, program, &all, &envs)
}

pub(crate) fn host_command(config: &Config, args: &[&str]) -> Command {
    let program = if config.guest_exec.is_some() {
        Path::new("/work/smolrunners/tests/vm/bin/smolrunners")
    } else {
        &config.agent
    };
    let host_state = if config.guest_exec.is_some() {
        config.guest_state_dir.join("host")
    } else {
        config.state_dir.join("host")
    };
    let runtime = if config.guest_exec.is_some() {
        PathBuf::from("/opt/runtime")
    } else {
        config.runtime_dir.clone()
    };
    let launcher = if config.guest_exec.is_some() {
        PathBuf::from("/work/smolrunners/tests/vm/bin/smolrunners-launcher")
    } else {
        config.launcher.clone()
    };
    let mut all = vec![
        "host".to_owned(),
        "--state-dir".to_owned(),
        host_state.display().to_string(),
        "--runtime-dir".to_owned(),
        runtime.display().to_string(),
        "--launcher".to_owned(),
        launcher.display().to_string(),
        "--uid".to_owned(),
        "60000".to_owned(),
    ];
    all.extend(args.iter().map(|value| (*value).to_owned()));
    let envs: [(&str, String); 0] = [];
    runner(config, program, &all, &envs)
}

struct Supervisor {
    config: Config,
    lifecycle: Lifecycle,
    ledger: Ledger,
    history: Arc<Mutex<History>>,
    runtime: RuntimeState,
    samplers: BTreeMap<String, Sampler>,
    audit_log: PathBuf,
    runtime_path: PathBuf,
}

impl Supervisor {
    fn new(config: Config) -> Result<Self, String> {
        fs::create_dir_all(&config.state_dir).map_err(|error| format!("create state dir: {error}"))?;
        let audit_log = config.state_dir.join("coordinator.log");
        let runtime_path = config.state_dir.join("runtime.json");
        let db_path = config.state_dir.join("coordinator.sqlite");
        for directory in [
            config.state_dir.join("home/.config"),
            config.state_dir.join("home/.cache"),
            config.state_dir.join("tmp"),
            config.state_dir.join("host"),
        ] {
            fs::create_dir_all(&directory)
                .map_err(|error| format!("create {}: {error}", directory.display()))?;
        }
        let store = Store::open(&db_path).map_err(|error| format!("open store: {error}"))?;
        let mut lifecycle =
            Lifecycle::from_store(store).map_err(|error| format!("open lifecycle: {error}"))?;
        let mut ledger =
            Ledger::open_path(&db_path).map_err(|error| format!("open ledger: {error}"))?;
        let history = History::open(&db_path).map_err(|error| format!("open history: {error}"))?;
        let now = Tick::new(now_ms());
        let host_key = HostKey::new(config.host_key.clone());
        ledger
            .register_host(HostCapacity {
                host_key: host_key.clone(),
                incarnation: HostIncarnation::new(1),
                hostname: config.host_key.clone(),
                arch: smolrunners_core::domain::Arch::Arm64,
                total_vcpu: config.host_vcpu,
                total_memory_gib: config.host_memory_gib,
                total_disk_bytes: 64 * 1024 * 1024 * 1024,
                headroom_vcpu: 0,
                headroom_memory_gib: 0,
                headroom_disk_bytes: 0,
                vmm_overhead: VmmOverhead::zero(),
                admission: HostAdmission::Eligible,
                cache_zone: None,
                last_contact: now,
            })
            .map_err(|error| format!("register host: {error}"))?;
        let runtime = if runtime_path.exists() {
            let raw = fs::read_to_string(&runtime_path)
                .map_err(|error| format!("read runtime state: {error}"))?;
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            RuntimeState::default()
        };
        let recovered = lifecycle
            .recover(&format!("t={}", now_ms()))
            .map_err(|error| format!("recover: {error}"))?;
        audit(&audit_log, &format!("BOOT host={} recovered={}", config.host_key, recovered.len()));
        let history = Arc::new(Mutex::new(history));
        let mut samplers = BTreeMap::new();
        for job in runtime.jobs.values() {
            if job.machine.is_some() && job.status != "done" {
                match effects::start_sampling(&config, job, history.clone(), &audit_log) {
                    Ok(sampler) => {
                        audit(&audit_log, &format!("REATTACH job={} attempt={} machine={}",
                            job.job_id, job.execution_attempt_id,
                            job.machine.as_deref().unwrap_or("unknown")));
                        samplers.insert(job.job_id.clone(), sampler);
                    }
                    Err(error) => audit(&audit_log, &format!("REATTACH_FAILED job={} error={error}", job.job_id)),
                }
            }
        }
        Ok(Self {
            config,
            lifecycle,
            ledger,
            history,
            runtime,
            samplers,
            audit_log,
            runtime_path,
        })
    }

    fn persist_runtime(&self) {
        let raw = match serde_json::to_string_pretty(&self.runtime) {
            Ok(raw) => raw,
            Err(_) => return,
        };
        let temporary = self.runtime_path.with_extension("json.tmp");
        if fs::write(&temporary, raw).is_ok() {
            let _ = fs::rename(&temporary, &self.runtime_path);
        }
    }

    fn kill_stale_adapter(&self) {
        let pid_path = self.config.state_dir.join("adapter.pid");
        if let Ok(text) = fs::read_to_string(&pid_path) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                if pid > 0 {
                    let _ = Command::new("kill")
                        .arg("-9")
                        .arg(pid.to_string())
                        .status();
                    audit(&self.audit_log, &format!("REAP_ADAPTER pid={pid}"));
                }
            }
        }
        let _ = fs::remove_file(pid_path);
    }

    fn run(&mut self) -> Result<(), String> {
        self.kill_stale_adapter();
        for scenario in self.config.scenarios.clone() {
            self.run_scenario(&scenario)?;
        }
        audit(&self.audit_log, "COMPLETE");
        Ok(())
    }

    fn run_scenario(&mut self, scenario: &str) -> Result<(), String> {
        audit(&self.audit_log, &format!("SCENARIO name={scenario}"));
        let log_path = self.config.state_dir.join(format!("adapter-{scenario}.log"));
        let log = File::create(&log_path).map_err(|error| format!("adapter log: {error}"))?;
        let error_log = log
            .try_clone()
            .map_err(|error| format!("clone adapter log: {error}"))?;
        let (adapter_program, adapter_scenarios) = if self.config.guest_exec.is_some() {
            (
                PathBuf::from("/work/smolrunners/tests/cli/bin/smolrunners-adapter"),
                self.config.guest_state_dir.join("scenarios"),
            )
        } else {
            (self.config.adapter.clone(), self.config.scenarios_dir.clone())
        };
        let mut child = runner(
            &self.config,
            &adapter_program,
            &[],
            &[("SMOLRUNNERS_SCENARIOS_DIR", adapter_scenarios.display().to_string())],
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(error_log)
        .spawn()
        .map_err(|error| format!("spawn adapter: {error}"))?;
        if let Ok(pid) = i32::try_from(child.id()) {
            let _ = fs::write(self.config.state_dir.join("adapter.pid"), pid.to_string());
        }
        let mut writer = child.stdin.take().ok_or("adapter stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("adapter stdout unavailable")?;
        let mut reader = BufReader::new(stdout);
        let start = Frame::Run {
            schema_version: SCHEMA_VERSION,
            id: format!("run-{scenario}"),
            scenario: scenario.to_owned(),
        };
        proto::write_frame(&mut writer, &start).map_err(|error| error.to_string())?;
        loop {
            let frame = match read_frame(&mut reader) {
                Ok(frame) => frame,
                Err(error) => {
                    let _ = child.kill();
                    return Err(format!("read adapter frame: {error}"));
                }
            };
            if frame.is_ack() {
                let _ = child.kill();
                return Err("adapter sent an ack frame to the coordinator".to_owned());
            }
            if matches!(
                &frame,
                Frame::Session {
                    session_event,
                    ..
                } if session_event == "adapter_stopped"
            ) {
                break;
            }
            self.dispatch(&frame, scenario, &mut writer)?;
        }
        let _ = child.wait();
        let _ = fs::remove_file(self.config.state_dir.join("adapter.pid"));
        Ok(())
    }

    fn dispatch<W: Write>(&mut self, frame: &Frame, scenario: &str, writer: &mut W) -> Result<(), String> {
        let job_id = effects::frame_job_id(frame).map(str::to_owned);
        if let (Frame::JobAvailable { job_id, request_id, labels, .. }, Some(key)) =
            (frame, job_id.as_deref())
        {
            self.ensure_job(key, job_id.as_str(), request_id.get(), labels, scenario);
        }
        let sink_job = match frame {
            Frame::AcquireOutcome { request_ids, .. } => request_ids
                .iter()
                .find_map(|request| {
                    self.runtime
                        .jobs
                        .values()
                        .find(|job| job.request_id == request.get())
                        .map(|job| job.job_id.clone())
                })
                .unwrap_or_else(|| frame.id().to_owned()),
            _ => job_id.clone().unwrap_or_else(|| frame.id().to_owned()),
        };
        let now = format!("t={}", now_ms());
        let (submitted, reserved) = {
            let mut sink = effects::HostSink::new(
                &self.config,
                &mut self.ledger,
                &mut self.runtime,
                self.history.clone(),
                &self.audit_log,
                sink_job,
            );
            let outcome = self
                .lifecycle
                .submit_with_kill(frame, &now, &mut sink, None)
                .map_err(|error| format!("submit {}: {error}", frame.kind()))?;
            let submitted = match outcome {
                SubmitOutcome::Completed(submitted) => submitted,
                SubmitOutcome::Killed => return Err("unexpected kill point".to_owned()),
            };
            (submitted, sink.reservation_id())
        };
        audit(
            &self.audit_log,
            &format!(
                "EVENT kind={} id={} decision={} recorded={} replay={}",
                frame.kind(),
                frame.id(),
                submitted.decision.as_str(),
                matches!(&submitted.ack, Frame::Ack { recorded: true, .. }),
                submitted.replayed
            ),
        );
        if submitted.decision.as_str() != "accept" {
            emit_ack(writer, &submitted)?;
            return Ok(());
        }

        match frame {
            Frame::JobAvailable { .. } => {
                if let Some(id) = reserved {
                    if let Some(key) = job_id.as_deref() {
                        if let Some(job) = self.runtime.jobs.get_mut(key) {
                            job.reservation_id = Some(id);
                            job.status = "reserved".to_owned();
                        }
                        self.persist_runtime();
                    }
                }
            }
            Frame::JobAssigned { .. } if submitted.replayed => {
                let key = job_id.as_deref().unwrap_or("unknown");
                let deadline = Instant::now() + Duration::from_secs(20);
                let mut observed = 0u64;
                while Instant::now() < deadline {
                    observed = self
                        .samplers
                        .get(key)
                        .map(|sampler| sampler.samples.load(Ordering::Relaxed))
                        .unwrap_or(0);
                    if observed >= 2 {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                audit(
                    &self.audit_log,
                    &format!(
                        "RESUME_WAIT job={key} reason=coordinator_restart samples={observed} ready={}",
                        observed >= 2
                    ),
                );
            }
            Frame::JobAssigned { .. } if !submitted.replayed => {
                let key = job_id.clone().ok_or("assignment has no job id")?;
                let event_id = format!("provision-{key}");
                let requested = [
                    smolrunners_core::lifecycle::Effect::CreateGuest,
                    smolrunners_core::lifecycle::Effect::StartSampling,
                ];
                {
                    let mut sink = effects::HostSink::new(
                        &self.config,
                        &mut self.ledger,
                        &mut self.runtime,
                        self.history.clone(),
                        &self.audit_log,
                        key.clone(),
                    );
                    let outcome = self
                        .lifecycle
                        .submit_effects(&event_id, &requested, Some(ExecutionState::Busy), &now, &mut sink, None)
                        .map_err(|error| format!("launch {key}: {error}"))?;
                    match outcome {
                        SubmitOutcome::Completed(_) => {}
                        SubmitOutcome::Killed => return Err("unexpected kill point".to_owned()),
                    }
                    if let Some(sampler) = sink.sampler.take() {
                        self.samplers.insert(key.clone(), sampler);
                    }
                    let reservation = sink.reservation_id();
                    if let Some(job) = self.runtime.jobs.get_mut(&key) {
                        job.reservation_id = reservation;
                    }
                }
                if let Some(job) = self.runtime.jobs.get_mut(&key) {
                    job.status = "busy".to_owned();
                }
                self.persist_runtime();
                let workload_job = self.runtime.jobs.get(&key).cloned();
                if let Some(job) = workload_job {
                    effects::run_workload(&self.config, &job, &self.audit_log)?;
                }
            }
            Frame::JobCompleted { result, .. } => {
                if let Some(key) = job_id.as_deref() {
                    if let Some(mut sampler) = self.samplers.remove(key) {
                        sampler.stop();
                    }

                    let attempt = self
                        .runtime
                        .jobs
                        .get(key)
                        .map(|job| job.execution_attempt_id)
                        .unwrap_or(0);
                    if let Some(job) = self.runtime.jobs.get_mut(key) {
                        job.status = "done".to_owned();
                    }
                    self.persist_runtime();
                    audit(&self.audit_log, &format!("JOB_DONE job={key} attempt={attempt} result={result}"));
                }
            }
            _ => {}
        }
        emit_ack(writer, &submitted)
    }

    fn ensure_job(&mut self, key: &str, job_id: &str, request_id: i64, labels: &[String], scenario: &str) {
        if self.runtime.jobs.contains_key(key) {
            return;
        }
        let attempt_id = self.next_attempt_id();
        let host_id = 1;
        let job = JobRuntime {
            job_id: key.to_owned(),
            protocol_job_id: job_id.to_owned(),
            request_id,
            execution_attempt_id: attempt_id,
            host_id,
            profile: self.config.profile.clone(),
            machine: None,
            reservation_id: None,
            reservation_sequence: None,
            status: "demanded".to_owned(),
            scenario: scenario.to_owned(),
        };
        audit(
            &self.audit_log,
            &format!(
                "DEMAND job={key} attempt={attempt_id} host={host_id} labels={}",
                labels.join(",")
            ),
        );
        self.runtime.jobs.insert(key.to_owned(), job);
        self.persist_runtime();
    }

    fn next_attempt_id(&self) -> i64 {
        let base = now_ms() as i64;
        self.runtime
            .jobs
            .values()
            .map(|job| job.execution_attempt_id)
            .max()
            .unwrap_or(0)
            .max(base)
            + 1
    }
}

pub fn serve(args: &[String]) -> Result<(), String> {
    let config = parse_args(args)?;
    let listen = config.listen.clone();
    let api_state = config.state_dir.join("api");
    thread::spawn(move || {
        let api_args = vec![
            "--listen".to_owned(),
            listen,
            "--state-dir".to_owned(),
            api_state.display().to_string(),
        ];
        let _ = crate::api::serve(&api_args);
    });
    let mut supervisor = Supervisor::new(config)?;
    supervisor.run()
}
