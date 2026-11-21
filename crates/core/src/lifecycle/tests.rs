use std::collections::HashSet;

use crate::domain::{ProtocolJobId, RunnerId, RunnerName, RunnerRequestId};
use crate::lifecycle::state::ExecutionState;
use crate::lifecycle::{
    CrashHarness, Decision, Effect, KillPoint, Lifecycle, ObservationKind, RecordingSink,
};
use crate::proto::{Frame, SCHEMA_VERSION};
use crate::store::test_support::{remove_db, temp_db_path};

fn job(value: &str) -> ProtocolJobId {
    ProtocolJobId::new(value).expect("job id")
}

fn request(value: i64) -> RunnerRequestId {
    RunnerRequestId::new(value).expect("request id")
}

fn available(event: &str, request_id: i64, job_id: &str) -> Frame {
    Frame::JobAvailable {
        schema_version: SCHEMA_VERSION,
        id: event.to_owned(),
        request_id: request(request_id),
        job_id: job(job_id),
        labels: Vec::new(),
        reason: None,
    }
}

fn assigned(event: &str, request_id: i64, job_id: &str, reason: Option<String>) -> Frame {
    Frame::JobAssigned {
        schema_version: SCHEMA_VERSION,
        id: event.to_owned(),
        request_id: request(request_id),
        job_id: job(job_id),
        runner_id: RunnerId::new(7).expect("runner id"),
        reason,
    }
}

fn acquire(event: &str, request_id: i64) -> Frame {
    Frame::AcquireOutcome {
        schema_version: SCHEMA_VERSION,
        id: event.to_owned(),
        request_ids: vec![request(request_id)],
        acquired_ids: vec![request(request_id)],
        reason: None,
    }
}

#[test]
fn lifecycle_states_are_all_distinguishable() {
    let states = [
        ExecutionState::Reserved,
        ExecutionState::Provisioning,
        ExecutionState::GuestReady,
        ExecutionState::Registering,
        ExecutionState::Idle,
        ExecutionState::Busy,
        ExecutionState::Finalizing,
        ExecutionState::Terminating,
        ExecutionState::Removed,
        ExecutionState::Failed,
        ExecutionState::Uncertain,
    ];
    let mut names = HashSet::new();
    for state in states {
        assert!(names.insert(state.as_str()), "duplicate state {state}");
        assert_eq!(ExecutionState::parse(state.as_str()), Some(state));
    }
    assert_eq!(states.len(), 11);
    assert!(ExecutionState::Removed.is_terminal());
    assert!(ExecutionState::Failed.is_terminal());
    assert!(!ExecutionState::Uncertain.is_terminal());
}

#[test]
fn lifecycle_normal_path_visits_every_state() {
    let path = temp_db_path("lifecycle_states");
    let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
    let mut sink = RecordingSink::new();
    lifecycle
        .submit(&available("evt-walk", 1, "job-walk"), "t0", &mut sink)
        .expect("submit offer");
    assert_eq!(lifecycle.state_of("job-walk").expect("read state"), Some(ExecutionState::Reserved));

    for state in [
        ExecutionState::Provisioning,
        ExecutionState::GuestReady,
        ExecutionState::Registering,
        ExecutionState::Idle,
        ExecutionState::Busy,
        ExecutionState::Finalizing,
        ExecutionState::Terminating,
        ExecutionState::Removed,
    ] {
        lifecycle
            .transition("job-walk", state, "t1")
            .unwrap_or_else(|error| panic!("transition to {state}: {error}"));
        assert_eq!(lifecycle.state_of("job-walk").expect("read state"), Some(state));
    }
    assert!(lifecycle
        .transition("job-walk", ExecutionState::Busy, "t2")
        .is_err());
    remove_db(&path);
}

#[test]
fn lifecycle_failed_and_uncertain_states_are_reachable() {
    let path = temp_db_path("lifecycle_observations");
    let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
    let mut sink = RecordingSink::new();
    let rejected = assigned("evt-reject", 2, "job-reject", Some("assignment_expired".to_owned()));
    let submitted = lifecycle.submit(&rejected, "t0", &mut sink).expect("submit rejection");
    assert_eq!(submitted.decision, Decision::Reject);
    assert_eq!(lifecycle.state_of("job-reject").expect("read state"), Some(ExecutionState::Failed));
    let observation = lifecycle.observation("evt-reject").expect("read observation");
    assert_eq!(observation.map(|row| row.kind), Some(ObservationKind::Failed.as_str().to_owned()));

    let mut uncertain_sink = RecordingSink::with_uncertain(Effect::ReserveCapacity);
    let uncertain = lifecycle
        .submit(&available("evt-uncertain", 3, "job-uncertain"), "t1", &mut uncertain_sink)
        .expect("submit uncertain offer");
    assert_eq!(uncertain.decision, Decision::Uncertain);
    assert_eq!(
        lifecycle.state_of("job-uncertain").expect("read state"),
        Some(ExecutionState::Uncertain)
    );
    let observation = lifecycle.observation("evt-uncertain").expect("read observation");
    assert_eq!(observation.map(|row| row.kind), Some(ObservationKind::Uncertain.as_str().to_owned()));
    remove_db(&path);
}

#[test]
fn lifecycle_redelivery_is_idempotent_by_event_id() {
    let path = temp_db_path("lifecycle_redelivery");
    let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
    let frame = acquire("evt-acquire", 11);
    let mut first_sink = RecordingSink::new();
    let first = lifecycle.submit(&frame, "t0", &mut first_sink).expect("first submit");
    assert!(!first.replayed);
    assert_eq!(first.effects, vec![Effect::AcquireJobs]);
    assert_eq!(first.decision, Decision::Accept);

    let mut second_sink = RecordingSink::new();
    let second = lifecycle.submit(&frame, "t1", &mut second_sink).expect("redelivery");
    assert!(second.replayed);
    assert!(second.effects.is_empty());
    assert_eq!(second.decision, first.decision);
    assert_eq!(second.ack, first.ack);
    assert_eq!(first_sink.performed().len(), 1);
    assert!(second_sink.performed().is_empty());
    assert_eq!(lifecycle.acquisition_count().expect("count"), 1);
    remove_db(&path);
}

#[test]
fn lifecycle_failed_effect_is_not_acked_as_success() {
    let path = temp_db_path("lifecycle_no_ack_on_failure");
    let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
    let mut sink = RecordingSink::with_uncertain(Effect::AcquireJobs);
    let submitted = lifecycle
        .submit(&acquire("evt-unknown-outcome", 12), "t0", &mut sink)
        .expect("submit");
    assert_eq!(submitted.decision, Decision::Uncertain);
    assert_eq!(submitted.ack, Frame::Ack {
        schema_version: SCHEMA_VERSION,
        id: "ack-evt-unknown-outcome".to_owned(),
        event_id: "evt-unknown-outcome".to_owned(),
        decision: "uncertain".to_owned(),
        recorded: true,
    });
    assert!(lifecycle.observation("evt-unknown-outcome").expect("observation").is_some());
    remove_db(&path);
}

#[test]
fn lifecycle_consumes_every_frame_kind() {
    let path = temp_db_path("lifecycle_kinds");
    let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
    let runner_name = RunnerName::new("runner-1").expect("runner name");
    let frames = vec![
        Frame::Run {
            schema_version: SCHEMA_VERSION,
            id: "evt-run".to_owned(),
            scenario: "redelivery".to_owned(),
        },
        Frame::Capacity {
            schema_version: SCHEMA_VERSION,
            id: "evt-capacity".to_owned(),
            assigned_jobs: 1,
            reported_capacity: 1,
        },
        available("evt-offer", 21, "job-kinds"),
        assigned("evt-assigned", 22, "job-kinds", None),
        Frame::JobStarted {
            schema_version: SCHEMA_VERSION,
            id: "evt-started".to_owned(),
            request_id: request(22),
            job_id: job("job-kinds"),
            runner_name,
        },
        Frame::JobCompleted {
            schema_version: SCHEMA_VERSION,
            id: "evt-completed".to_owned(),
            request_id: request(22),
            job_id: job("job-kinds"),
            result: "success".to_owned(),
        },
        acquire("evt-acquired", 21),
        Frame::Session {
            schema_version: SCHEMA_VERSION,
            id: "evt-session".to_owned(),
            session_event: "key_rotated".to_owned(),
            retry_after_ms: None,
            reason: None,
        },
        Frame::VmDelete {
            schema_version: SCHEMA_VERSION,
            id: "evt-delete".to_owned(),
            job_id: job("job-kinds"),
        },
        Frame::Ack {
            schema_version: SCHEMA_VERSION,
            id: "evt-ack".to_owned(),
            event_id: "evt-offer".to_owned(),
            decision: "accept".to_owned(),
            recorded: true,
        },
    ];
    let mut sink = RecordingSink::new();
    for frame in &frames {
        let submitted = lifecycle.submit(frame, "t0", &mut sink).expect("submit frame");
        assert!(submitted.ack.is_ack(), "no ack for {}", frame.kind());
        assert_eq!(submitted.ack.correlation_id(), frame.id());
        assert_eq!(submitted.decision, Decision::Accept, "{}", frame.kind());
    }
    let kinds: HashSet<&str> = frames.iter().map(Frame::kind).collect();
    assert_eq!(kinds.len(), 10);
    remove_db(&path);
}

#[test]
fn lifecycle_crash_around_acquisition_never_acks_unpersisted_work() {
    let path = temp_db_path("lifecycle_crash_acquire");
    let frame = acquire("evt-crash-acquire", 31);
    let harness = CrashHarness::new(&path);
    let report = harness
        .run(&frame, KillPoint::After(Effect::AcquireJobs))
        .expect("crash after acquire");
    assert!(report.killed);
    assert!(!report.acked_at_kill);
    assert!(!report.acquisition_persisted_at_kill);
    assert!(report.recovered.contains(&"evt-crash-acquire".to_owned()));
    assert!(report.replay.replayed);
    assert_eq!(report.replay.decision, Decision::Uncertain);
    assert_eq!(report.effects_after_restart, 0);

    let path = temp_db_path("lifecycle_crash_before_acquire");
    let frame = acquire("evt-crash-before", 32);
    let harness = CrashHarness::new(&path);
    let report = harness
        .run(&frame, KillPoint::Before(Effect::AcquireJobs))
        .expect("crash before acquire");
    assert!(report.killed);
    assert!(!report.acked_at_kill);
    assert!(!report.acquisition_persisted_at_kill);
    assert_eq!(report.replay.decision, Decision::Uncertain);
    assert_eq!(report.effects_after_restart, 0);
    remove_db(&path);
}

#[test]
fn lifecycle_crash_before_and_after_every_effect() {
    for effect in Effect::ALL {
        for kill in [KillPoint::Before(effect), KillPoint::After(effect)] {
            let path = temp_db_path(&format!("lifecycle_crash_{effect:?}_{kill:?}"));
            let harness = CrashHarness::new(&path);
            let event_id = format!("evt-{}", effect.as_str());
            let report = if effect == Effect::ReserveCapacity {
                harness
                    .run(&available(&event_id, 41, &event_id), kill)
                    .expect("crash with offer")
            } else if effect == Effect::AcquireJobs {
                harness.run(&acquire(&event_id, 42), kill).expect("crash with acquire")
            } else {
                harness
                    .run_effects(&event_id, &[effect], None, kill)
                    .expect("crash with host progress")
            };
            assert!(report.killed, "{effect:?} {kill:?} did not kill");
            assert!(!report.acked_at_kill, "{effect:?} {kill:?} acked before commit");
            assert!(report.recovered.contains(&event_id), "{effect:?} {kill:?} not reconciled");
            assert_eq!(report.replay.decision, Decision::Uncertain, "{effect:?} {kill:?} replayed");
            assert!(report.replay.replayed, "{effect:?} {kill:?} was not a replay");
            assert_eq!(report.effects_after_restart, 0, "{effect:?} {kill:?} replayed an effect");
            assert!(!acquisition_exists(&path, &event_id).expect("check acquisition"));
            remove_db(&path);
        }
    }
}

fn acquisition_exists(path: &std::path::Path, event_id: &str) -> crate::lifecycle::Result<bool> {
    Lifecycle::open(path)?.acquisition_exists(event_id)
}
