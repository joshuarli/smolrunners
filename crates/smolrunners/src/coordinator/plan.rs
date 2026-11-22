use crate::api::JobRecord;

pub(crate) const SCALE_DOWN_GRACE_MS: i64 = 30_000;

pub(crate) fn runners_to_start(max_concurrency: u32, assigned_jobs: i64, live: usize) -> usize {
    let target = assigned_jobs.clamp(0, i64::from(max_concurrency)) as usize;
    target.saturating_sub(live)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopReason {
    Cancelled,
    IdleTimeout,
    ScaledDown,
    CompletionGrace,
}

impl StopReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::IdleTimeout => "idle_timeout",
            Self::ScaledDown => "scaled_down",
            Self::CompletionGrace => "completion_grace",
        }
    }
}

pub(crate) struct StopPolicy {
    pub(crate) now_ms: i64,
    pub(crate) assigned_jobs: i64,
    pub(crate) idle_timeout_ms: i64,
    pub(crate) completion_grace_ms: i64,
}

pub(crate) fn stops(policy: &StopPolicy, live: &[JobRecord]) -> Vec<(i64, StopReason)> {
    let mut decided = Vec::new();
    let mut excess = (live.len() as i64).saturating_sub(policy.assigned_jobs.max(0));
    for job in live {
        if job.machine.is_none() {
            continue;
        }
        if job.cancel_requested {
            decided.push((job.id, StopReason::Cancelled));
            continue;
        }
        if let Some(completed) = job.github_completed_at_ms {
            if policy.now_ms - completed >= policy.completion_grace_ms {
                decided.push((job.id, StopReason::CompletionGrace));
            }
            continue;
        }
        if job.state != "idle" {
            continue;
        }
        let Some(idle_since) = job.idle_since_ms else {
            continue;
        };
        let idle_for = policy.now_ms - idle_since;
        if idle_for >= policy.idle_timeout_ms {
            decided.push((job.id, StopReason::IdleTimeout));
            excess -= 1;
        } else if excess > 0 && idle_for >= SCALE_DOWN_GRACE_MS {
            decided.push((job.id, StopReason::ScaledDown));
            excess -= 1;
        }
    }
    decided
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunnerExit {
    Code(i32),
    Signal,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostEvent {
    Claim(String),
    Ready,
    RunnerStarted,
    RunnerExit(RunnerExit),
    Failed(String),
    Removed,
}

pub(crate) fn parse_host_line(line: &str) -> Option<HostEvent> {
    let line = line.trim();
    if let Some(machine) = line.strip_prefix("CLAIM ") {
        return Some(HostEvent::Claim(machine.trim().to_owned()));
    }
    if line.starts_with("READY ") {
        return Some(HostEvent::Ready);
    }
    if line.starts_with("RUNNER_STARTED ") {
        return Some(HostEvent::RunnerStarted);
    }
    if let Some(rest) = line.strip_prefix("RUNNER_EXIT ") {
        let exit = match rest.trim() {
            "canceled" => RunnerExit::Canceled,
            "signal" => RunnerExit::Signal,
            other => match other.strip_prefix("code=").and_then(|code| code.parse().ok()) {
                Some(code) => RunnerExit::Code(code),
                None => return Some(HostEvent::Failed(format!("unreadable runner exit {other}"))),
            },
        };
        return Some(HostEvent::RunnerExit(exit));
    }
    if let Some(message) = line.strip_prefix("RUNNER_FAILED ") {
        return Some(HostEvent::Failed(message.to_owned()));
    }
    if let Some(message) = line.strip_prefix("INELIGIBLE ") {
        return Some(HostEvent::Failed(format!("host ineligible: {message}")));
    }
    if let Some(message) = line.strip_prefix("host: ") {
        return Some(HostEvent::Failed(message.to_owned()));
    }
    if line.starts_with("REMOVED ") {
        return Some(HostEvent::Removed);
    }
    None
}

pub(crate) struct Outcome<'a> {
    pub(crate) cancel_requested: bool,
    pub(crate) github_result: Option<&'a str>,
    pub(crate) runner_exit: Option<&'a RunnerExit>,
    pub(crate) failure: Option<&'a str>,
}

pub(crate) fn final_state(outcome: &Outcome) -> (&'static str, String) {
    if outcome.cancel_requested {
        return ("cancelled", "cancelled by operator".to_owned());
    }
    if let Some(result) = outcome.github_result {
        let state = match result.to_ascii_lowercase().as_str() {
            "succeeded" | "success" => "done",
            "canceled" | "cancelled" => "cancelled",
            _ => "failed",
        };
        return (state, result.to_owned());
    }
    if let Some(failure) = outcome.failure {
        return ("failed", failure.to_owned());
    }
    match outcome.runner_exit {
        Some(RunnerExit::Code(0)) => ("done", "runner exited 0".to_owned()),
        Some(RunnerExit::Code(code)) => ("failed", format!("runner exited {code}")),
        Some(RunnerExit::Signal) => ("failed", "runner killed by a signal".to_owned()),
        Some(RunnerExit::Canceled) => ("cancelled", "runner stopped by the coordinator".to_owned()),
        None => ("failed", "runner never started".to_owned()),
    }
}
