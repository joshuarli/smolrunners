use std::fmt;

use serde::Deserialize;

pub const HOOK_JOB_STARTED: &str = "ACTIONS_RUNNER_HOOK_JOB_STARTED";
pub const HOOK_JOB_COMPLETED: &str = "ACTIONS_RUNNER_HOOK_JOB_COMPLETED";
pub const HOOK_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    JobStarted,
    JobCompleted,
}

impl HookKind {
    pub fn from_env_name(name: &str) -> Option<Self> {
        match name {
            HOOK_JOB_STARTED => Some(Self::JobStarted),
            HOOK_JOB_COMPLETED => Some(Self::JobCompleted),
            _ => None,
        }
    }

    pub const fn env_name(self) -> &'static str {
        match self {
            Self::JobStarted => HOOK_JOB_STARTED,
            Self::JobCompleted => HOOK_JOB_COMPLETED,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookNotification {
    pub kind: HookKind,
    pub observed_at_ms: i64,
    pub exit_status: Option<i32>,
}

impl HookNotification {
    pub fn deadline_ms(&self) -> i64 {
        self.observed_at_ms.saturating_add(HOOK_TIMEOUT_MS as i64)
    }

    pub fn within_budget_at(&self, now_ms: i64) -> bool {
        now_ms <= self.deadline_ms()
    }
}

#[derive(Debug, Default)]
pub struct HookJournal {
    notifications: Vec<HookNotification>,
}

impl HookJournal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, notification: HookNotification) {
        self.notifications.push(notification);
    }

    pub fn notifications(&self) -> &[HookNotification] {
        &self.notifications
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobMetadata {
    pub run_id: u64,
    pub job_id: u64,
    pub run_attempt: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityPendingInterval {
    pub started_at_ms: i64,
    pub attached_at_ms: i64,
}

impl IdentityPendingInterval {
    pub fn from_notifications(
        started: &HookNotification,
        attached: &HookNotification,
    ) -> Option<Self> {
        if started.kind != HookKind::JobStarted || attached.kind != HookKind::JobCompleted {
            return None;
        }
        if attached.observed_at_ms < started.observed_at_ms {
            return None;
        }
        Some(Self {
            started_at_ms: started.observed_at_ms,
            attached_at_ms: attached.observed_at_ms,
        })
    }

    pub fn contains(&self, wall_unix_ms: i64) -> bool {
        wall_unix_ms >= self.started_at_ms && wall_unix_ms <= self.attached_at_ms
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingSample {
    pub sequence: u64,
    pub monotonic_ns: u64,
    pub wall_unix_ms: i64,
    pub metric: String,
    pub value: f64,
    pub identity: Option<JobMetadata>,
}

pub fn attach_identity(
    samples: &mut [PendingSample],
    interval: &IdentityPendingInterval,
    metadata: &JobMetadata,
) -> usize {
    let mut relabeled = 0;
    for sample in samples {
        if sample.identity.is_none() && interval.contains(sample.wall_unix_ms) {
            sample.identity = Some(metadata.clone());
            relabeled += 1;
        }
    }
    relabeled
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowJobsRequest {
    pub owner: String,
    pub repo: String,
    pub run_id: u64,
}

impl WorkflowJobsRequest {
    pub fn path(&self) -> String {
        format!(
            "/repos/{}/{}/actions/runs/{}/jobs",
            self.owner, self.repo, self.run_id
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum WorkflowJobsError {
    Transport(String),
    Decode(String),
    MissingTimestamp(String),
}

impl fmt::Display for WorkflowJobsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(message) => write!(f, "workflow-jobs transport: {message}"),
            Self::Decode(message) => write!(f, "workflow-jobs decode: {message}"),
            Self::MissingTimestamp(value) => {
                write!(f, "workflow-jobs missing timestamp: {value}")
            }
        }
    }
}

impl std::error::Error for WorkflowJobsError {}

pub trait WorkflowJobsTransport {
    fn fetch(&self, request: &WorkflowJobsRequest) -> Result<String, String>;
}

impl<T: WorkflowJobsTransport + ?Sized> WorkflowJobsTransport for &T {
    fn fetch(&self, request: &WorkflowJobsRequest) -> Result<String, String> {
        (**self).fetch(request)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOverlay {
    pub job_id: u64,
    pub step_number: u64,
    pub name: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
}

pub fn overlays_within(
    overlays: &[StepOverlay],
    interval: &IdentityPendingInterval,
) -> Vec<StepOverlay> {
    overlays
        .iter()
        .filter(|overlay| {
            overlay.started_at_ms >= interval.started_at_ms
                && overlay.completed_at_ms <= interval.attached_at_ms
        })
        .cloned()
        .collect()
}

#[derive(Debug, Deserialize)]
struct RecordedJobsResponse {
    jobs: Vec<RecordedJob>,
}

#[derive(Debug, Deserialize)]
struct RecordedJob {
    id: u64,
    completed_at: Option<String>,
    steps: Vec<RecordedStep>,
}

#[derive(Debug, Deserialize)]
struct RecordedStep {
    number: u64,
    name: String,
    started_at: Option<String>,
    completed_at: Option<String>,
}

pub struct WorkflowJobsClient<T: WorkflowJobsTransport> {
    transport: T,
}

impl<T: WorkflowJobsTransport> WorkflowJobsClient<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn step_overlays(
        &self,
        request: &WorkflowJobsRequest,
    ) -> Result<Vec<StepOverlay>, WorkflowJobsError> {
        let body = self
            .transport
            .fetch(request)
            .map_err(WorkflowJobsError::Transport)?;
        let response: RecordedJobsResponse =
            serde_json::from_str(&body).map_err(|error| WorkflowJobsError::Decode(error.to_string()))?;
        let mut overlays = Vec::new();
        for job in response.jobs {
            if job.completed_at.is_none() {
                continue;
            }
            for step in job.steps {
                let (started, completed) = match (&step.started_at, &step.completed_at) {
                    (Some(started), Some(completed)) => (started, completed),
                    _ => continue,
                };
                let started_at_ms = parse_rfc3339_ms(started)
                    .ok_or_else(|| WorkflowJobsError::MissingTimestamp(started.clone()))?;
                let completed_at_ms = parse_rfc3339_ms(completed)
                    .ok_or_else(|| WorkflowJobsError::MissingTimestamp(completed.clone()))?;
                overlays.push(StepOverlay {
                    job_id: job.id,
                    step_number: step.number,
                    name: step.name,
                    started_at_ms,
                    completed_at_ms,
                });
            }
        }
        overlays.sort_by_key(|overlay| (overlay.job_id, overlay.step_number));
        Ok(overlays)
    }
}

pub fn parse_rfc3339_ms(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let year = digits(bytes, 0, 4)? as i64;
    expect_byte(bytes.get(4), b'-')?;
    let month = digits(bytes, 5, 2)? as i64;
    expect_byte(bytes.get(7), b'-')?;
    let day = digits(bytes, 8, 2)? as i64;
    expect_byte(bytes.get(10), b'T')?;
    let hour = digits(bytes, 11, 2)? as i64;
    expect_byte(bytes.get(13), b':')?;
    let minute = digits(bytes, 14, 2)? as i64;
    expect_byte(bytes.get(16), b':')?;
    let second = digits(bytes, 17, 2)? as i64;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }

    let mut cursor = 19;
    let mut millis = 0i64;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor == start {
            return None;
        }
        let fraction = &value[start..cursor];
        let padded: String = fraction
            .chars()
            .take(3)
            .chain(std::iter::repeat('0').take(3))
            .take(3)
            .collect();
        millis = padded.parse().ok()?;
    }

    let offset_seconds = match bytes.get(cursor) {
        Some(b'Z') if cursor + 1 == bytes.len() => 0i64,
        Some(sign @ (b'+' | b'-')) => {
            if cursor + 6 != bytes.len() {
                return None;
            }
            let offset_hour = digits(bytes, cursor + 1, 2)? as i64;
            expect_byte(bytes.get(cursor + 3), b':')?;
            let offset_minute = digits(bytes, cursor + 4, 2)? as i64;
            if offset_hour > 23 || offset_minute > 59 {
                return None;
            }
            let magnitude = offset_hour * 3_600 + offset_minute * 60;
            if *sign == b'+' {
                magnitude
            } else {
                -magnitude
            }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let epoch_seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds;
    Some(epoch_seconds * 1_000 + millis)
}

fn digits(bytes: &[u8], start: usize, count: usize) -> Option<u32> {
    let slice = bytes.get(start..start + count)?;
    let mut value = 0u32;
    for byte in slice {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(byte - b'0');
    }
    Some(value)
}

fn expect_byte(actual: Option<&u8>, expected: u8) -> Option<()> {
    if actual == Some(&expected) {
        Some(())
    } else {
        None
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let shifted_year = year - i64::from(month <= 2);
    let era = if shifted_year >= 0 {
        shifted_year
    } else {
        shifted_year - 399
    } / 400;
    let year_of_era = shifted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const RECORDED_JOBS: &str = r#"
    {
      "total_count": 2,
      "jobs": [
        {
          "id": 5551001,
          "run_id": 99001,
          "name": "build",
          "status": "completed",
          "conclusion": "success",
          "started_at": "2026-11-21T12:00:00Z",
          "completed_at": "2026-11-21T12:00:30Z",
          "steps": [
            {"number": 1, "name": "Set up job", "status": "completed",
             "started_at": "2026-11-21T12:00:00Z", "completed_at": "2026-11-21T12:00:03Z"},
            {"number": 2, "name": "Checkout", "status": "completed",
             "started_at": "2026-11-21T12:00:03.500Z", "completed_at": "2026-11-21T12:00:08Z"},
            {"number": 3, "name": "Build", "status": "completed",
             "started_at": "2026-11-21T12:00:08Z", "completed_at": "2026-11-21T12:00:27.250Z"},
            {"number": 4, "name": "Pending step", "status": "skipped",
             "started_at": null, "completed_at": null}
          ]
        },
        {
          "id": 5551002,
          "run_id": 99001,
          "name": "test",
          "status": "in_progress",
          "conclusion": null,
          "started_at": "2026-11-21T12:00:31Z",
          "completed_at": null,
          "steps": [
            {"number": 1, "name": "Set up job", "status": "completed",
             "started_at": "2026-11-21T12:00:31Z", "completed_at": "2026-11-21T12:00:33Z"}
          ]
        }
      ]
    }
    "#;

    struct RecordedTransport {
        body: String,
        requested: RefCell<Vec<WorkflowJobsRequest>>,
    }

    impl RecordedTransport {
        fn new(body: impl Into<String>) -> Self {
            Self {
                body: body.into(),
                requested: RefCell::new(Vec::new()),
            }
        }
    }

    impl WorkflowJobsTransport for RecordedTransport {
        fn fetch(&self, request: &WorkflowJobsRequest) -> Result<String, String> {
            self.requested.borrow_mut().push(request.clone());
            Ok(self.body.clone())
        }
    }

    struct FailingTransport;

    impl WorkflowJobsTransport for FailingTransport {
        fn fetch(&self, _request: &WorkflowJobsRequest) -> Result<String, String> {
            Err("offline".to_owned())
        }
    }

    fn notification(kind: HookKind, observed_at_ms: i64) -> HookNotification {
        HookNotification {
            kind,
            observed_at_ms,
            exit_status: None,
        }
    }

    fn sample(wall_unix_ms: i64) -> PendingSample {
        PendingSample {
            sequence: wall_unix_ms as u64,
            monotonic_ns: wall_unix_ms as u64 * 1_000_000,
            wall_unix_ms,
            metric: "cpu.busy_cores".to_owned(),
            value: wall_unix_ms as f64 / 1_000.0,
            identity: None,
        }
    }

    #[test]
    fn telemetry_hooks_are_bounded_local_notifications() {
        assert_eq!(HookKind::from_env_name(HOOK_JOB_STARTED), Some(HookKind::JobStarted));
        assert_eq!(HookKind::from_env_name(HOOK_JOB_COMPLETED), Some(HookKind::JobCompleted));
        assert_eq!(HookKind::from_env_name("SMOLRUNNERS_VM_DESTROY"), None);
        assert_eq!(HookKind::JobStarted.env_name(), HOOK_JOB_STARTED);

        let started = notification(HookKind::JobStarted, 1_000);
        assert_eq!(started.deadline_ms(), 1_000 + HOOK_TIMEOUT_MS as i64);
        assert!(started.within_budget_at(2_000));
        assert!(started.within_budget_at(3_000));
        assert!(!started.within_budget_at(3_001));

        let mut journal = HookJournal::new();
        journal.record(started);
        journal.record(notification(HookKind::JobCompleted, 9_000));
        assert_eq!(journal.notifications().len(), 2);
        assert_eq!(journal.notifications()[0].kind, HookKind::JobStarted);
        assert_eq!(journal.notifications()[1].kind, HookKind::JobCompleted);
    }

    #[test]
    fn telemetry_hooks_attach_identity_in_place_without_moving_samples() {
        let started = notification(HookKind::JobStarted, 1_000);
        let completed = notification(HookKind::JobCompleted, 9_000);
        let interval = IdentityPendingInterval::from_notifications(&started, &completed)
            .expect("pending interval");
        assert!(interval.contains(1_000));
        assert!(interval.contains(5_000));
        assert!(interval.contains(9_000));
        assert!(!interval.contains(999));
        assert!(!interval.contains(9_001));
        assert!(IdentityPendingInterval::from_notifications(&completed, &started).is_none());

        let mut samples = vec![sample(999), sample(1_500), sample(5_000), sample(8_500), sample(9_001)];
        let before: Vec<(u64, u64, i64, f64)> = samples
            .iter()
            .map(|item| (item.sequence, item.monotonic_ns, item.wall_unix_ms, item.value))
            .collect();
        let metadata = JobMetadata {
            run_id: 99001,
            job_id: 5551001,
            run_attempt: 2,
        };
        assert_eq!(attach_identity(&mut samples, &interval, &metadata), 3);
        assert_eq!(samples[0].identity, None);
        assert_eq!(samples[1].identity, Some(metadata.clone()));
        assert_eq!(samples[2].identity, Some(metadata.clone()));
        assert_eq!(samples[3].identity, Some(metadata.clone()));
        assert_eq!(samples[4].identity, None);
        let after: Vec<(u64, u64, i64, f64)> = samples
            .iter()
            .map(|item| (item.sequence, item.monotonic_ns, item.wall_unix_ms, item.value))
            .collect();
        assert_eq!(before, after);
        assert_eq!(attach_identity(&mut samples, &interval, &metadata), 0);
    }

    #[test]
    fn telemetry_hooks_parse_rfc3339_capture_and_offset_timestamps() {
        assert_eq!(parse_rfc3339_ms("2024-11-30T20:53:20.007Z"), Some(1_733_000_000_007));
        assert_eq!(parse_rfc3339_ms("2024-11-30T20:53:20Z"), Some(1_733_000_000_000));
        assert_eq!(parse_rfc3339_ms("2024-11-30T15:53:20-05:00"), Some(1_733_000_000_000));
        assert_eq!(parse_rfc3339_ms("2024-12-01T01:53:20+05:00"), Some(1_733_000_000_000));
        assert_eq!(parse_rfc3339_ms("2026-11-21T12:00:03.500Z"), Some(parse_rfc3339_ms("2026-11-21T12:00:03Z").unwrap() + 500));
        assert_eq!(parse_rfc3339_ms(""), None);
        assert_eq!(parse_rfc3339_ms("not-a-time"), None);
        assert_eq!(parse_rfc3339_ms("2024-11-30T20:53:20+25:00"), None);
        assert_eq!(parse_rfc3339_ms("2024-13-30T20:53:20Z"), None);
    }

    #[test]
    fn telemetry_hooks_attach_step_times_after_completion_from_recorded_response() {
        let transport = RecordedTransport::new(RECORDED_JOBS);
        let client = WorkflowJobsClient::new(&transport);
        let request = WorkflowJobsRequest {
            owner: "o".to_owned(),
            repo: "r".to_owned(),
            run_id: 99001,
        };
        let overlays = client.step_overlays(&request).expect("step overlays");
        assert_eq!(transport.requested.borrow()[0].path(), "/repos/o/r/actions/runs/99001/jobs");

        assert_eq!(overlays.len(), 3);
        assert!(overlays.iter().all(|overlay| overlay.job_id == 5551001));
        assert_eq!(
            overlays.iter().map(|overlay| overlay.step_number).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(overlays[0].name, "Set up job");
        assert_eq!(overlays[2].name, "Build");
        for overlay in &overlays {
            assert!(overlay.completed_at_ms > overlay.started_at_ms);
        }
        let checkout = &overlays[1];
        assert_eq!(checkout.started_at_ms % 1_000, 500);
        let build = &overlays[2];
        assert_eq!(build.completed_at_ms % 1_000, 250);
        assert!(!overlays.iter().any(|overlay| overlay.name == "Pending step"));

        let interval = IdentityPendingInterval {
            started_at_ms: parse_rfc3339_ms("2026-11-21T11:59:59Z").unwrap(),
            attached_at_ms: parse_rfc3339_ms("2026-11-21T12:01:00Z").unwrap(),
        };
        let attached = overlays_within(&overlays, &interval);
        assert_eq!(attached.len(), 3);
        let narrow = IdentityPendingInterval {
            started_at_ms: parse_rfc3339_ms("2026-11-21T12:00:04Z").unwrap(),
            attached_at_ms: parse_rfc3339_ms("2026-11-21T12:00:30Z").unwrap(),
        };
        assert_eq!(overlays_within(&overlays, &narrow).len(), 1);
    }

    #[test]
    fn telemetry_hooks_report_recorded_response_and_transport_failures() {
        let client = WorkflowJobsClient::new(RecordedTransport::new("{not json"));
        let request = WorkflowJobsRequest {
            owner: "o".to_owned(),
            repo: "r".to_owned(),
            run_id: 1,
        };
        assert!(matches!(client.step_overlays(&request), Err(WorkflowJobsError::Decode(_))));

        let offline = WorkflowJobsClient::new(FailingTransport);
        assert_eq!(
            offline.step_overlays(&request),
            Err(WorkflowJobsError::Transport("offline".to_owned()))
        );

        let missing = RecordedTransport::new(
            r#"{"jobs":[{"id":1,"completed_at":"2026-11-21T12:00:01Z","steps":[
                {"number":1,"name":"bad","started_at":"oops","completed_at":"2026-11-21T12:00:01Z"}]}]}"#,
        );
        let client = WorkflowJobsClient::new(&missing);
        assert!(matches!(client.step_overlays(&request), Err(WorkflowJobsError::MissingTimestamp(_))));
    }
}
