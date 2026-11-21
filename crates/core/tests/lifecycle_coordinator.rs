use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use smolrunners_core::domain::{ProtocolJobId, RunnerRequestId};
use smolrunners_core::lifecycle::{
    Decision, Effect, EffectResult, EffectSink, KillPoint, Lifecycle, SubmitOutcome,
};
use smolrunners_core::proto::{Frame, SCHEMA_VERSION};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

fn temp_db(label: &str) -> PathBuf {
    let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "smolrunners-lifecycle-{label}-{}-{sequence}.sqlite",
        std::process::id()
    ))
}

fn remove_db(path: &PathBuf) {
    let name = path.file_name().expect("file name").to_string_lossy();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut candidate = path.clone();
        if !suffix.is_empty() {
            candidate.set_file_name(format!("{name}{suffix}"));
        }
        let _ = std::fs::remove_file(candidate);
    }
}

#[derive(Default)]
struct CountingSink {
    calls: usize,
}

impl EffectSink for CountingSink {
    fn perform(
        &mut self,
        _effect: Effect,
    ) -> smolrunners_core::lifecycle::Result<EffectResult> {
        self.calls += 1;
        Ok(EffectResult::Committed)
    }
}

fn acquire_frame(event_id: &str, request_id: i64) -> Frame {
    Frame::AcquireOutcome {
        schema_version: SCHEMA_VERSION,
        id: event_id.to_owned(),
        request_ids: vec![RunnerRequestId::new(request_id).expect("request id")],
        acquired_ids: vec![RunnerRequestId::new(request_id).expect("request id")],
        reason: None,
    }
}

fn offer_frame(event_id: &str, job_id: &str) -> Frame {
    Frame::JobAvailable {
        schema_version: SCHEMA_VERSION,
        id: event_id.to_owned(),
        request_id: RunnerRequestId::new(71).expect("request id"),
        job_id: ProtocolJobId::new(job_id).expect("job id"),
        labels: Vec::new(),
        reason: None,
    }
}

#[test]
fn lifecycle_coordinator_restart_preserves_acquisition_and_never_replays() {
    let path = temp_db("restart");
    let frame = acquire_frame("evt-coord-acquire", 71);
    {
        let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
        let mut sink = CountingSink::default();
        let submitted = lifecycle.submit(&frame, "t0", &mut sink).expect("submit acquire");
        assert_eq!(submitted.decision, Decision::Accept);
        assert_eq!(sink.calls, 1);
    }

    let mut lifecycle = Lifecycle::open(&path).expect("reopen lifecycle");
    assert!(lifecycle.acquisition_exists("evt-coord-acquire").expect("acquisition survived restart"));
    let mut sink = CountingSink::default();
    let redelivered = lifecycle.submit(&frame, "t1", &mut sink).expect("redeliver acquire");
    assert!(redelivered.replayed);
    assert!(redelivered.effects.is_empty());
    assert_eq!(sink.calls, 0);
    remove_db(&path);
}

#[test]
fn lifecycle_coordinator_reconciles_interrupted_effect() {
    let path = temp_db("interrupt");
    {
        let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
        let mut sink = CountingSink::default();
        let outcome = lifecycle
            .submit_effects(
                "evt-provision",
                &[Effect::CreateGuest],
                None,
                "t0",
                &mut sink,
                Some(KillPoint::After(Effect::CreateGuest)),
            )
            .expect("submit with kill");
        assert_eq!(outcome, SubmitOutcome::Killed);
        assert_eq!(sink.calls, 1);
    }

    let mut lifecycle = Lifecycle::open(&path).expect("reopen lifecycle");
    let recovered = lifecycle.recover("t1").expect("recover");
    assert_eq!(recovered, vec!["evt-provision".to_owned()]);
    let mut sink = CountingSink::default();
    let replay = lifecycle
        .submit_effects("evt-provision", &[Effect::CreateGuest], None, "t2", &mut sink, None)
        .expect("replay after restart");
    let replay = match replay {
        SubmitOutcome::Completed(submitted) => submitted,
        SubmitOutcome::Killed => panic!("no kill point requested"),
    };
    assert!(replay.replayed);
    assert_eq!(replay.decision, Decision::Uncertain);
    assert_eq!(sink.calls, 0);
    remove_db(&path);
}

#[test]
fn lifecycle_coordinator_distinguishes_failed_offer() {
    let path = temp_db("failed");
    let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
    let mut frame = offer_frame("evt-failed-offer", "job-failed");
    if let Frame::JobAvailable { reason, .. } = &mut frame {
        *reason = Some("assignment_expired".to_owned());
    }
    let mut sink = CountingSink::default();
    let submitted = lifecycle.submit(&frame, "t0", &mut sink).expect("submit rejection");
    assert_eq!(submitted.decision, Decision::Reject);
    assert_eq!(sink.calls, 0);
    assert!(lifecycle.observation("evt-failed-offer").expect("observation").is_some());
    remove_db(&path);
}
