use std::path::PathBuf;

use super::config::PilotConfig;
use super::plan::{self, HostEvent, Outcome, RunnerExit, StopPolicy, StopReason};
use crate::api::{EffectiveLimits, NewExecution, State};

fn limits() -> EffectiveLimits {
    EffectiveLimits {
        guest_vcpu: 2,
        guest_memory_gib: 4,
        storage_gib: 20,
        host_cpu_max: "225000 100000".to_owned(),
        host_memory_max_bytes: 4_831_838_208,
        host_pids_max: 512,
    }
}

fn live_jobs(states: &[(&str, Option<i64>)]) -> Vec<crate::api::JobRecord> {
    let dir = std::env::temp_dir().join(format!("smolrunners-plan-{}-{}", std::process::id(), states.len()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut state = State::open(&dir).expect("state");
    let mut jobs = Vec::new();
    for (index, (name, idle_since)) in states.iter().enumerate() {
        let mut job = state
            .create_execution(
                NewExecution {
                    profile: "p".to_owned(),
                    labels: vec!["smol".to_owned()],
                    host_id: 1,
                    limits: limits(),
                    reservation_sequence: Some(index as u64),
                    runner_name_prefix: "smol-test".to_owned(),
                },
                0,
            )
            .expect("execution");
        job.state = (*name).to_owned();
        job.idle_since_ms = *idle_since;
        job.machine = Some(format!("sr-{index}"));
        jobs.push(job);
    }
    let _ = std::fs::remove_dir_all(&dir);
    jobs
}

#[test]
fn scaling_starts_only_the_missing_runners_up_to_concurrency() {
    assert_eq!(plan::runners_to_start(1, 3, 0), 1);
    assert_eq!(plan::runners_to_start(1, 3, 1), 0);
    assert_eq!(plan::runners_to_start(4, 2, 1), 1);
    assert_eq!(plan::runners_to_start(4, 0, 0), 0);
    assert_eq!(plan::runners_to_start(4, -1, 0), 0);
}

#[test]
fn idle_runners_are_stopped_after_scale_down_grace_or_idle_timeout_but_busy_ones_are_not() {
    let jobs = live_jobs(&[("idle", Some(0)), ("busy", None), ("idle", Some(55_000))]);
    let policy = StopPolicy {
        now_ms: 60_000,
        assigned_jobs: 1,
        idle_timeout_ms: 900_000,
        completion_grace_ms: 120_000,
    };
    let decided = plan::stops(&policy, &jobs);
    assert_eq!(decided, vec![(jobs[0].id, StopReason::ScaledDown)]);

    let timed_out = StopPolicy { now_ms: 1_000_000, assigned_jobs: 3, ..policy };
    let decided = plan::stops(&timed_out, &jobs);
    assert_eq!(
        decided,
        vec![(jobs[0].id, StopReason::IdleTimeout), (jobs[2].id, StopReason::IdleTimeout)]
    );
}

#[test]
fn cancellation_and_completion_grace_stop_runners_regardless_of_demand() {
    let mut jobs = live_jobs(&[("busy", None), ("busy", None)]);
    jobs[0].cancel_requested = true;
    jobs[1].github_completed_at_ms = Some(0);
    let policy = StopPolicy {
        now_ms: 200_000,
        assigned_jobs: 5,
        idle_timeout_ms: 900_000,
        completion_grace_ms: 120_000,
    };
    assert_eq!(
        plan::stops(&policy, &jobs),
        vec![(jobs[0].id, StopReason::Cancelled), (jobs[1].id, StopReason::CompletionGrace)]
    );
}

#[test]
fn host_agent_lines_map_to_lifecycle_events() {
    assert_eq!(plan::parse_host_line("CLAIM sr-1-x7\n"), Some(HostEvent::Claim("sr-1-x7".to_owned())));
    assert_eq!(plan::parse_host_line("RUNNER_STARTED sr-1-x7"), Some(HostEvent::RunnerStarted));
    assert_eq!(plan::parse_host_line("RUNNER_EXIT code=3"), Some(HostEvent::RunnerExit(RunnerExit::Code(3))));
    assert_eq!(plan::parse_host_line("RUNNER_EXIT canceled"), Some(HostEvent::RunnerExit(RunnerExit::Canceled)));
    assert_eq!(
        plan::parse_host_line("host: storage admission refused: full"),
        Some(HostEvent::Failed("storage admission refused: full".to_owned()))
    );
    assert_eq!(plan::parse_host_line("REMOVED sr-1-x7"), Some(HostEvent::Removed));
    assert_eq!(plan::parse_host_line("Pulling image..."), None);
}

#[test]
fn final_state_prefers_operator_cancel_then_github_result_then_runner_exit() {
    let exit_ok = RunnerExit::Code(0);
    let exit_bad = RunnerExit::Code(1);
    let outcome = |cancel, github, exit, failure| Outcome {
        cancel_requested: cancel,
        github_result: github,
        runner_exit: exit,
        failure,
    };
    assert_eq!(plan::final_state(&outcome(true, Some("succeeded"), Some(&exit_ok), None)).0, "cancelled");
    assert_eq!(plan::final_state(&outcome(false, Some("Failed"), Some(&exit_ok), None)).0, "failed");
    assert_eq!(plan::final_state(&outcome(false, Some("canceled"), None, None)).0, "cancelled");
    assert_eq!(plan::final_state(&outcome(false, None, Some(&exit_ok), None)).0, "done");
    assert_eq!(plan::final_state(&outcome(false, None, Some(&exit_bad), None)).0, "failed");
    let (state, result) = plan::final_state(&outcome(false, None, None, Some("jit_request: 403")));
    assert_eq!((state, result.as_str()), ("failed", "jit_request: 403"));
}

fn pilot(extra: &str, listen: &str, host_memory_gib: u32) -> String {
    format!(
        r#"
state_dir = "/var/lib/smolrunners/coordinator"
listen = "{listen}"
max_concurrency = 1
{extra}

[adapter]
program = "/opt/smolrunners/bin/smolrunners-adapter"
mode = "github"
github_config = "/etc/smolrunners/github.json"

[host]
key = "threadripper"
arch = "x64"
vcpu = 32
memory_gib = {host_memory_gib}
disk_gib = 500
agent = "/opt/smolrunners/bin/smolrunners"
launcher = "/opt/smolrunners/bin/smolrunners-launcher"
runtime_dir = "/opt/smolrunners/runtime"
state_dir = "/var/lib/smolrunners/state"

[profile]
name = "smol-debian13-x64-4c8g"
labels = ["smol-debian13-x64-4c8g"]
vcpu = 4
memory_gib = 8
storage_gib = 40
overlay_gib = 10
image = "/var/lib/smolrunners/images/debian13/"
entrypoint = "/opt/smolrunners/bin/run-job"

[vmm_overhead]
memory_mib = 512
vcpu_milli = 250
"#
    )
}

fn load(text: &str) -> Result<PilotConfig, String> {
    let path: PathBuf = std::env::temp_dir().join(format!("smolrunners-pilot-{}-{}.toml", std::process::id(), text.len()));
    std::fs::write(&path, text).expect("write pilot");
    let loaded = PilotConfig::load(&path);
    let _ = std::fs::remove_file(&path);
    loaded
}

#[test]
fn one_profile_drives_guest_shape_host_cgroup_limits_and_displayed_limits() {
    let config = load(&pilot("", "127.0.0.1:8787", 64)).expect("valid pilot");
    let limits = config.limits().expect("limits");
    assert_eq!((limits.guest_vcpu, limits.guest_memory_gib), (4, 8));
    assert_eq!(limits.host_cpu_max, "425000 100000");
    assert_eq!(limits.host_memory_max_bytes, (8 * 1024 + 512) * 1024 * 1024);
    assert_eq!(limits.storage_gib, 40);
}

#[test]
fn the_pilot_refuses_a_non_loopback_api_and_a_host_too_small_for_the_profile() {
    let remote = load(&pilot("", "0.0.0.0:8787", 64)).expect_err("non-loopback listen");
    assert!(remote.contains("loopback"), "{remote}");
    let small = load(&pilot("", "127.0.0.1:8787", 8)).expect_err("host too small");
    assert!(small.contains("memory_gib"), "{small}");
    let unknown = load(&pilot("surprise = 1", "127.0.0.1:8787", 64)).expect_err("unknown key");
    assert!(unknown.contains("surprise"), "{unknown}");
}
