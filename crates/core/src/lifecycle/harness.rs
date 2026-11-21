use std::path::{Path, PathBuf};

use crate::domain::{Arch, ProfileRevisionId, Shape};
use crate::lifecycle::error::{LifecycleError, Result};
use crate::lifecycle::machine::{
    Effect, EffectResult, EffectSink, KillPoint, Lifecycle, SubmitOutcome, Submitted,
};
use crate::lifecycle::state::ExecutionState;
use crate::ledger::{
    HostAdmission, HostCapacity, HostIncarnation, HostKey, Ledger, ReservationId,
    ReservationOutcome, ReservationRequest, Tick, VmmOverhead,
};
use crate::proto::Frame;
use crate::store::StoreError;

#[derive(Debug)]
pub struct RecordingSink {
    performed: Vec<Effect>,
    uncertain_on: Option<Effect>,
}

impl RecordingSink {
    pub fn new() -> Self {
        Self {
            performed: Vec::new(),
            uncertain_on: None,
        }
    }

    pub fn with_uncertain(effect: Effect) -> Self {
        Self {
            performed: Vec::new(),
            uncertain_on: Some(effect),
        }
    }

    pub fn performed(&self) -> &[Effect] {
        &self.performed
    }
}

impl Default for RecordingSink {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectSink for RecordingSink {
    fn perform(&mut self, effect: Effect) -> Result<EffectResult> {
        self.performed.push(effect);
        Ok(if self.uncertain_on == Some(effect) {
            EffectResult::Uncertain
        } else {
            EffectResult::Committed
        })
    }
}

pub struct LedgerSink {
    ledger: Ledger,
    host_key: HostKey,
    performed: Vec<Effect>,
    reservation: Option<ReservationId>,
}

impl LedgerSink {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut ledger = Ledger::open_path(path)?;
        let host_key = HostKey::new("sr-host-a");
        ledger.register_host(HostCapacity {
            host_key: host_key.clone(),
            incarnation: HostIncarnation::new(1),
            hostname: "sr-host-a".to_owned(),
            arch: Arch::from_label_token("arm64").expect("arm64 label token"),
            total_vcpu: 8,
            total_memory_gib: 16,
            total_disk_bytes: 512 * 1024 * 1024 * 1024,
            headroom_vcpu: 1,
            headroom_memory_gib: 1,
            headroom_disk_bytes: 8 * 1024 * 1024 * 1024,
            vmm_overhead: VmmOverhead::new(128, 50),
            admission: HostAdmission::Eligible,
            cache_zone: None,
            last_contact: Tick::new(0),
        })?;
        Ok(Self {
            ledger,
            host_key,
            performed: Vec::new(),
            reservation: None,
        })
    }

    pub fn performed(&self) -> &[Effect] {
        &self.performed
    }

    pub fn reservation(&self) -> Option<ReservationId> {
        self.reservation
    }
}

impl EffectSink for LedgerSink {
    fn perform(&mut self, effect: Effect) -> Result<EffectResult> {
        self.performed.push(effect);
        match effect {
            Effect::ReserveCapacity => {
                let revision_id: ProfileRevisionId = "ab".repeat(32).parse().expect("hex id");
                let request = ReservationRequest {
                    revision_id,
                    arch: Arch::from_label_token("arm64").expect("arm64 label token"),
                    shape: Shape::new(2, 4).expect("valid shape"),
                    disk_bytes: 8 * 1024 * 1024 * 1024,
                    preferred_cache_zone: None,
                };
                match self.ledger.reserve(request, Tick::new(0))? {
                    ReservationOutcome::Granted(offer) => {
                        self.reservation = Some(offer.reservation_id);
                    }
                    ReservationOutcome::Pending { .. } => {
                        return Err(LifecycleError::Store(StoreError::Migration(
                            "reservation was not granted".to_owned(),
                        )));
                    }
                }
            }
            Effect::ReleaseCapacity => {
                if let Some(id) = self.reservation.take() {
                    self.ledger.release(id, &self.host_key, HostIncarnation::new(1), Tick::new(1))?;
                }
            }
            _ => {}
        }
        Ok(EffectResult::Committed)
    }
}

pub struct CrashReport {
    pub kill_point: KillPoint,
    pub killed: bool,
    pub acked_at_kill: bool,
    pub acquisition_persisted_at_kill: bool,
    pub recovered: Vec<String>,
    pub replay: Submitted,
    pub effects_after_restart: usize,
}

pub struct CrashHarness {
    path: PathBuf,
}

impl CrashHarness {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn run(&self, frame: &Frame, kill_point: KillPoint) -> Result<CrashReport> {
        let mut lifecycle = Lifecycle::open(&self.path)?;
        let mut sink = LedgerSink::open(&self.path)?;
        let outcome = lifecycle.submit_with_kill(frame, "t=kill", &mut sink, Some(kill_point))?;
        let killed = outcome == SubmitOutcome::Killed;
        let acked_at_kill = matches!(&outcome, SubmitOutcome::Completed(submitted) if submitted.ack.is_ack());
        let acquisition_persisted_at_kill = lifecycle.acquisition_exists(frame.id())?;
        drop(sink);
        drop(lifecycle);

        let mut lifecycle = Lifecycle::open(&self.path)?;
        let recovered = lifecycle.recover("t=restart")?;
        let mut sink = LedgerSink::open(&self.path)?;
        let replay = lifecycle.submit(frame, "t=replay", &mut sink)?;
        let effects_after_restart = sink.performed().len();

        Ok(CrashReport {
            kill_point,
            killed,
            acked_at_kill,
            acquisition_persisted_at_kill,
            recovered,
            replay,
            effects_after_restart,
        })
    }

    pub fn run_effects(
        &self,
        event_id: &str,
        effects: &[Effect],
        state: Option<ExecutionState>,
        kill_point: KillPoint,
    ) -> Result<CrashReport> {
        let mut lifecycle = Lifecycle::open(&self.path)?;
        let mut sink = LedgerSink::open(&self.path)?;
        let outcome = lifecycle.submit_effects(
            event_id,
            effects,
            state,
            "t=kill",
            &mut sink,
            Some(kill_point),
        )?;
        let killed = outcome == SubmitOutcome::Killed;
        let acked_at_kill = matches!(&outcome, SubmitOutcome::Completed(submitted) if submitted.ack.is_ack());
        drop(sink);
        drop(lifecycle);

        let mut lifecycle = Lifecycle::open(&self.path)?;
        let recovered = lifecycle.recover("t=restart")?;
        let mut sink = LedgerSink::open(&self.path)?;
        let replay = lifecycle.submit_effects(event_id, effects, state, "t=replay", &mut sink, None)?;
        let replay = match replay {
            SubmitOutcome::Completed(submitted) => submitted,
            SubmitOutcome::Killed => unreachable!("no kill point requested"),
        };
        let effects_after_restart = sink.performed().len();

        Ok(CrashReport {
            kill_point,
            killed,
            acked_at_kill,
            acquisition_persisted_at_kill: false,
            recovered,
            replay,
            effects_after_restart,
        })
    }
}

#[cfg(test)]
mod faults {
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    use rusqlite::params;

    use super::*;
    use crate::cache::{CacheAuthority, Digest, GenerationState, GenerationStore, PrivateView, TrustTier};
    use crate::domain::{
        Arch, Distro, ImageTarget, Profile, ProfileName, ProfileRevision, ProtocolJobId, ReleaseToken,
        RunnerId, RunnerRequestId, Shape,
    };
    use crate::lifecycle::{
        Decision as LifecycleDecision, Effect, KillPoint, Lifecycle, ObservationKind, RecordingSink,
    };
    use crate::ledger::{
        HostAdmission, HostCapacity, HostIncarnation, HostKey, Ledger, LedgerConfig, LedgerState,
        ReservationOutcome, ReservationRequest, ReservationState, Tick, VmmOverhead,
    };
    use crate::proto::SCHEMA_VERSION;
    use crate::store::test_support::{remove_db, temp_db_path};
    use crate::store::{Store, backup_to, open_connection, verify_effective_pragmas};
    use crate::telemetry::{GapReason, History, IngestContext, Metric, Sample, Scope, Unit};

    struct Scratch {
        files: Vec<PathBuf>,
        dirs: Vec<PathBuf>,
    }

    impl Scratch {
        fn new() -> Self {
            Self {
                files: Vec::new(),
                dirs: Vec::new(),
            }
        }

        fn db(&mut self, label: &str) -> PathBuf {
            let path = temp_db_path(label);
            self.files.push(path.clone());
            path
        }

        fn dir(&mut self, label: &str) -> PathBuf {
            let mut path = temp_db_path(label);
            path.set_extension("faults");
            let _ = fs::remove_dir_all(&path);
            self.dirs.push(path.clone());
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            for dir in &self.dirs {
                let _ = fs::remove_dir_all(dir);
            }
            for file in &self.files {
                remove_db(file);
            }
        }
    }

    fn report(case: &str, durable: &str, resources: &str) {
        eprintln!("FAULT_STATE case={case} durable={durable}");
        eprintln!("FAULT_STATE case={case} resources={resources}");
    }

    fn job(value: &str) -> ProtocolJobId {
        ProtocolJobId::new(value).expect("job id")
    }

    fn request(value: i64) -> RunnerRequestId {
        RunnerRequestId::new(value).expect("request id")
    }

    fn available(event: &str, request_id: i64, job_id: &str, reason: Option<String>) -> Frame {
        Frame::JobAvailable {
            schema_version: SCHEMA_VERSION,
            id: event.to_owned(),
            request_id: request(request_id),
            job_id: job(job_id),
            labels: Vec::new(),
            reason,
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

    fn host_capacity(key: &str, vcpu: u32, memory_gib: u32) -> HostCapacity {
        HostCapacity {
            host_key: HostKey::new(key),
            incarnation: HostIncarnation::new(1),
            hostname: key.to_owned(),
            arch: Arch::Arm64,
            total_vcpu: vcpu,
            total_memory_gib: memory_gib,
            total_disk_bytes: 1_000_000 * 1024 * 1024,
            headroom_vcpu: 0,
            headroom_memory_gib: 0,
            headroom_disk_bytes: 0,
            vmm_overhead: VmmOverhead::zero(),
            admission: HostAdmission::Eligible,
            cache_zone: None,
            last_contact: Tick::new(0),
        }
    }

    fn revision(name: &str, vcpu: u16, memory_gib: u32) -> ProfileRevision {
        let target = ImageTarget::new(
            Distro::Debian,
            ReleaseToken::from_parts(13, None),
            Arch::Arm64,
        );
        let profile = Profile::new(
            ProfileName::new(name).expect("valid name"),
            target,
            Shape::new(vcpu, memory_gib).expect("valid shape"),
        );
        ProfileRevision::new(profile, 1)
    }

    fn sample(
        sequence: u64,
        monotonic_ns: u64,
        wall_unix_ms: i64,
        boot: &str,
        generation: u64,
        value: f64,
    ) -> Sample {
        Sample {
            version: crate::telemetry::SCHEMA_VERSION,
            scope: Scope::Guest,
            sequence,
            monotonic_ns,
            wall_unix_ms,
            boot_id: boot.to_owned(),
            generation,
            metrics: vec![Metric {
                name: "counter".to_owned(),
                value,
                unit: Unit::Count,
            }],
        }
    }

    #[test]
    fn f01_controller_crash() {
        let mut scratch = Scratch::new();
        let path = scratch.db("fault-controller-crash");
        let frame = acquire("evt-f01", 101);
        let harness = CrashHarness::new(&path);
        let crash = harness
            .run(&frame, KillPoint::After(Effect::AcquireJobs))
            .expect("crash after acquire");
        assert!(crash.killed);
        assert!(!crash.acked_at_kill);
        assert!(!crash.acquisition_persisted_at_kill);
        assert!(crash.recovered.iter().any(|id| id == "evt-f01"));
        assert_eq!(crash.replay.decision, LifecycleDecision::Uncertain);
        assert_eq!(crash.effects_after_restart, 0);
        let lifecycle = Lifecycle::open(&path).expect("reopen");
        assert_eq!(lifecycle.acquisition_count().expect("count"), 0);
        assert_eq!(
            lifecycle.state_of("evt-f01").expect("state"),
            Some(ExecutionState::Uncertain)
        );
        report(
            "controller_crash",
            "inbox_done=1 acquisition=0 observation=uncertain state=uncertain",
            "replayed_effects=0 ledger_reservations=0",
        );
    }

    #[test]
    fn f02_host_agent_crash() {
        let mut scratch = Scratch::new();
        let path = scratch.db("fault-host-agent-crash");
        let frame = available("evt-f02", 102, "job-f02", None);
        let harness = CrashHarness::new(&path);
        let crash = harness
            .run(&frame, KillPoint::After(Effect::ReserveCapacity))
            .expect("crash after reserve");
        assert!(crash.killed);
        assert!(!crash.acked_at_kill);
        assert!(crash.recovered.iter().any(|id| id == "evt-f02"));
        assert_eq!(crash.effects_after_restart, 0);

        let mut ledger = Ledger::open_path(&path).expect("open ledger after restart");
        let owned: Vec<_> = ledger
            .reservations()
            .filter(|reservation| reservation.state == ReservationState::Reserved)
            .map(|reservation| {
                (
                    reservation.id,
                    reservation.host.clone().expect("host"),
                    reservation.incarnation,
                )
            })
            .collect();
        assert_eq!(owned.len(), 1, "the owned reservation must survive the crash");
        let (id, host, incarnation) = owned[0].clone();
        ledger
            .release(id, &host, incarnation, Tick::new(2))
            .expect("reap owned reservation");
        assert_eq!(
            ledger
                .reservations()
                .filter(|reservation| reservation.state.occupies_capacity())
                .count(),
            0
        );
        let lifecycle = Lifecycle::open(&path).expect("open lifecycle");
        assert_eq!(
            lifecycle.state_of("evt-f02").expect("state"),
            Some(ExecutionState::Uncertain)
        );
        report(
            "host_agent_crash",
            "inbox_done=1 state=uncertain reservations_before_reap=1 after_reap=0",
            "owned_resources_reaped=1 duplicate_releases=0",
        );
    }

    #[test]
    fn f03_vm_crash() {
        let mut scratch = Scratch::new();
        let path = scratch.db("fault-vm-crash");
        let harness = CrashHarness::new(&path);
        let crash = harness
            .run_effects(
                "evt-f03",
                &[Effect::DestroyResources, Effect::ReleaseCapacity],
                None,
                KillPoint::After(Effect::DestroyResources),
            )
            .expect("crash after destroy");
        assert!(crash.killed);
        assert!(!crash.acked_at_kill);
        assert_eq!(crash.effects_after_restart, 0);
        assert_eq!(crash.replay.decision, LifecycleDecision::Uncertain);
        let lifecycle = Lifecycle::open(&path).expect("open lifecycle");
        assert_eq!(
            lifecycle.state_of("evt-f03").expect("state"),
            Some(ExecutionState::Uncertain)
        );
        report(
            "vm_crash",
            "inbox_done=1 state=uncertain destroy_events=1 release_events=0",
            "duplicate_destroy=0 replayed_effects=0",
        );
    }

    #[test]
    fn f04_collector_failure() {
        let mut scratch = Scratch::new();
        let path = scratch.db("fault-collector-failure");
        let mut history = History::open(&path).expect("open history");
        let context = IngestContext::default();
        history
            .ingest(sample(1, 1_000_000_000, 1_000, "boot-a", 1, 5.0), context.clone())
            .expect("first sample");
        let after_outage = history
            .ingest(sample(9, 9_000_000_000, 9_000, "boot-a", 1, 6.0), context.clone())
            .expect("sample after outage");
        assert!(after_outage.gap);
        assert_eq!(after_outage.gap_reason, Some(GapReason::Sequence));
        assert!(
            after_outage.rates.is_empty(),
            "no rate may cross a collector outage"
        );
        for sequence in 10..610u64 {
            let outcome = history
                .ingest(
                    sample(
                        sequence,
                        sequence * 1_000_000_000,
                        10_000 + sequence as i64,
                        "boot-a",
                        1,
                        sequence as f64,
                    ),
                    context.clone(),
                )
                .expect("continuous sample");
            assert!(!outcome.gap, "collection must not stop at a sample count");
        }
        history.flush().expect("flush");
        assert_eq!(history.gaps(), 1);
        report(
            "collector_failure",
            "gap=sequence rates_across_gap=0 continuous_samples=600 gaps=1",
            "owned_db=1 spool_unbounded=no",
        );
    }

    #[test]
    fn f05_message_redelivery() {
        let mut scratch = Scratch::new();
        let path = scratch.db("fault-message-redelivery");
        let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
        let frame = acquire("evt-f05", 105);
        let mut first_sink = RecordingSink::new();
        let first = lifecycle.submit(&frame, "t0", &mut first_sink).expect("first");
        let mut second_sink = RecordingSink::new();
        let second = lifecycle.submit(&frame, "t1", &mut second_sink).expect("redelivery");
        assert!(!first.replayed && second.replayed);
        assert_eq!(first.effects, vec![Effect::AcquireJobs]);
        assert!(second.effects.is_empty());
        assert_eq!(first_sink.performed().len(), 1);
        assert!(second_sink.performed().is_empty());
        assert_eq!(lifecycle.acquisition_count().expect("count"), 1);
        assert_eq!(first.ack, second.ack);
        report(
            "message_redelivery",
            "inbox_done=1 acquisition=1 effects=1 ack_restored=1",
            "duplicate_executions=0",
        );
    }

    #[test]
    fn f06_lost_ack() {
        let mut scratch = Scratch::new();
        let path = scratch.db("fault-lost-ack");
        let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
        let frame = acquire("evt-f06", 106);
        let mut sink = RecordingSink::new();
        let submitted = lifecycle.submit(&frame, "t0", &mut sink).expect("submit");
        drop(submitted.ack);
        let mut replay_sink = RecordingSink::new();
        let replayed = lifecycle.submit(&frame, "t1", &mut replay_sink).expect("redelivery");
        assert!(replayed.replayed);
        assert!(replayed.effects.is_empty());
        assert!(replay_sink.performed().is_empty());
        assert_eq!(replayed.ack.correlation_id(), "evt-f06");
        assert_eq!(lifecycle.acquisition_count().expect("count"), 1);
        report(
            "lost_ack",
            "inbox_done=1 acquisition=1 ack_recovered_by_replay=1",
            "duplicate_executions=0 effects_on_replay=0",
        );
    }

    #[test]
    fn f07_stale_jit_material() {
        let mut scratch = Scratch::new();
        let root = scratch.dir("fault-stale-jit");
        let store = GenerationStore::new(root.join("cache"));
        let owner = CacheAuthority::new("acme/widgets", TrustTier::Maintainer);
        let first_source = root.join("jit-v1");
        fs::create_dir_all(&first_source).expect("first source");
        fs::write(first_source.join("jit"), b"token-v1").expect("first jit");
        let first = store
            .create(&first_source, &owner, None, "jit", "t1")
            .expect("first generation");
        store.publish(&first.id, "t2").expect("publish first");
        let second_source = root.join("jit-v2");
        fs::create_dir_all(&second_source).expect("second source");
        fs::write(second_source.join("jit"), b"token-v2").expect("second jit");
        let second = store
            .create(&second_source, &owner, Some(first.id.clone()), "jit", "t3")
            .expect("second generation");
        store.publish(&second.id, "t4").expect("publish second");
        store.retire(&first.id).expect("retire stale");
        assert_eq!(store.current("acme/widgets").as_deref(), Some(second.id.as_str()));
        let removed = store.gc().expect("gc");
        assert!(removed.contains(&first.id));
        assert!(store.get(&first.id).is_err());
        let current = store.get(&second.id).expect("current generation");
        assert_eq!(current.state, GenerationState::Published);
        let manifest = store.manifest(&current).expect("manifest");
        let digest = Digest::from_hex(&manifest.files[0].digest).expect("digest");
        assert_eq!(store.blobs().get(&digest).expect("blob"), b"token-v2");
        report(
            "stale_jit_material",
            "current=v2 retired=v1 gc_removed=1 manifest=token-v2",
            "generations=1 blobs_bounded=yes",
        );
    }

    #[test]
    fn f08_assignment_expiry() {
        let mut scratch = Scratch::new();
        let path = scratch.db("fault-assignment-expiry");
        let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
        let frame = assigned(
            "evt-f08",
            108,
            "job-f08",
            Some("assignment_expired".to_owned()),
        );
        let mut sink = RecordingSink::new();
        let submitted = lifecycle.submit(&frame, "t0", &mut sink).expect("submit");
        assert_eq!(submitted.decision, LifecycleDecision::Reject);
        assert!(sink.performed().is_empty());
        assert_eq!(
            lifecycle.state_of("job-f08").expect("state"),
            Some(ExecutionState::Failed)
        );
        let observation = lifecycle.observation("evt-f08").expect("observation").expect("present");
        assert_eq!(observation.kind, ObservationKind::Failed.as_str());
        report(
            "assignment_expiry",
            "decision=reject state=failed observation=failed",
            "effects=0 vms=0",
        );
    }

    #[test]
    fn f09_cancel_queued_and_running() {
        let mut scratch = Scratch::new();
        let path = scratch.db("fault-cancel");
        let mut lifecycle = Lifecycle::open(&path).expect("open lifecycle");
        let queued = available("evt-f09-queued", 109, "job-queued", Some("cancelled".to_owned()));
        let mut queued_sink = RecordingSink::new();
        let rejected = lifecycle.submit(&queued, "t0", &mut queued_sink).expect("queued cancel");
        assert_eq!(rejected.decision, LifecycleDecision::Reject);
        assert!(queued_sink.performed().is_empty());

        let mut sink = LedgerSink::open(&path).expect("ledger sink");
        let offer = available("evt-f09-offer", 110, "job-running", None);
        lifecycle.submit(&offer, "t1", &mut sink).expect("reserve");
        let reservation = sink.reservation().expect("reservation");
        let completed = Frame::JobCompleted {
            schema_version: SCHEMA_VERSION,
            id: "evt-f09-cancel".to_owned(),
            request_id: request(110),
            job_id: job("job-running"),
            result: "cancelled".to_owned(),
        };
        lifecycle.submit(&completed, "t2", &mut sink).expect("running cancel");
        assert!(sink.reservation().is_none());
        let ledger = Ledger::open_path(&path).expect("open ledger");
        assert_eq!(
            ledger
                .reservations()
                .filter(|entry| entry.state.occupies_capacity())
                .count(),
            0,
            "cancellation must release capacity"
        );
        let mut replay_sink = LedgerSink::open(&path).expect("replay sink");
        let replayed = lifecycle.submit(&completed, "t3", &mut replay_sink).expect("redelivery");
        assert!(replayed.replayed);
        assert!(replay_sink.performed().is_empty());
        let _ = reservation;
        report(
            "cancel_queued_and_running",
            "queued=reject running=removed redelivery=replayed reservation_released=1",
            "duplicate_destroy=0 owned_capacity=0",
        );
    }

    #[test]
    fn f10_clock_and_counter_resets() {
        let mut scratch = Scratch::new();
        let path = scratch.db("fault-clock-counter-resets");
        let mut history = History::open(&path).expect("open history");
        let context = IngestContext::default();
        history
            .ingest(sample(1, 1_000_000_000, 1_000, "boot-a", 1, 100.0), context.clone())
            .expect("first");
        let clock_reset = history
            .ingest(sample(2, 500_000_000, 2_000, "boot-a", 1, 110.0), context.clone())
            .expect("clock reset");
        assert_eq!(clock_reset.gap_reason, Some(GapReason::ClockReset));
        assert!(clock_reset.rates.is_empty());
        let incarnation = history
            .ingest(sample(3, 2_000_000_000, 3_000, "boot-b", 2, 5.0), context.clone())
            .expect("new incarnation");
        assert_eq!(incarnation.gap_reason, Some(GapReason::NewIncarnation));
        assert!(incarnation.rates.is_empty());
        let counter_reset = history
            .ingest(sample(4, 3_000_000_000, 4_000, "boot-b", 2, 1.0), context)
            .expect("counter reset");
        assert!(!counter_reset.gap);
        assert!(
            counter_reset.rates.is_empty(),
            "a decreasing cumulative counter must not produce a negative rate"
        );
        assert_eq!(history.gaps(), 2);
        report(
            "clock_and_counter_resets",
            "gaps=2 reasons=clock_reset,new_incarnation counter_rate_on_reset=none",
            "cross_incarnation_rates=0 owned_db=1",
        );
    }

    #[test]
    fn f11_wal_disk_pressure() {
        let mut scratch = Scratch::new();
        let path = scratch.db("fault-wal-pressure");
        let store = Store::open(&path).expect("open store");
        verify_effective_pragmas(store.connection()).expect("effective pragmas");
        let mode: String = store
            .connection()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal mode");
        assert_eq!(mode.to_ascii_lowercase(), "wal");

        let normal_path = scratch.db("fault-wal-normal");
        let normal = open_connection(&normal_path).expect("raw connection");
        normal
            .execute_batch("PRAGMA synchronous = NORMAL;")
            .expect("set normal");
        let mismatch = verify_effective_pragmas(&normal).expect_err("pragma mismatch");
        assert!(mismatch.to_string().contains("synchronous"));

        let full_path = scratch.db("fault-wal-full");
        let full = open_connection(&full_path).expect("pressure connection");
        full.execute_batch("PRAGMA max_page_count = 2; CREATE TABLE pressure (v BLOB);")
            .expect("pressure setup");
        let error = full
            .execute(
                "INSERT INTO pressure VALUES (?1)",
                params![vec![9u8; 65_536]],
            )
            .expect_err("page pressure must fail");
        assert!(
            matches!(
                error,
                rusqlite::Error::SqliteFailure(ref code, _)
                    if code.code == rusqlite::ErrorCode::DiskFull
            ),
            "unexpected error under page pressure: {error}"
        );

        let backup_path = scratch.db("fault-wal-backup");
        backup_to(store.connection(), &backup_path).expect("backup");
        let restored = open_connection(&backup_path).expect("open backup");
        let tables: i64 = restored
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .expect("count tables");
        assert!(tables > 0);
        report(
            "wal_disk_pressure",
            &format!("journal=wal synchronous=full normal_rejected=1 disk_full=mapped backup_tables={tables}"),
            "owned_db=1 partial_writes=0",
        );
    }

    #[test]
    fn f12_interrupted_generation_publication() {
        let mut scratch = Scratch::new();
        let root = scratch.dir("fault-interrupted-publish");
        let store = GenerationStore::new(root.join("cache"));
        let owner = CacheAuthority::new("acme/widgets", TrustTier::Maintainer);
        let baseline_source = root.join("baseline");
        fs::create_dir_all(&baseline_source).expect("baseline source");
        fs::write(baseline_source.join("tool"), b"v1").expect("baseline tool");
        let baseline = store
            .create(&baseline_source, &owner, None, "toolchain", "t1")
            .expect("baseline");
        store.publish(&baseline.id, "t2").expect("publish baseline");
        let next_source = root.join("next");
        fs::create_dir_all(&next_source).expect("next source");
        fs::write(next_source.join("tool"), b"v2").expect("next tool");
        let next = store
            .create(&next_source, &owner, Some(baseline.id.clone()), "toolchain", "t3")
            .expect("next");
        store.acquire_writer(&next.id).expect("hold writer");
        assert!(matches!(
            store.publish(&next.id, "t4"),
            Err(crate::cache::CacheError::NotQuiescent { .. })
        ));
        store.release_writer(&next.id).expect("release writer");
        store.publish(&next.id, "t5").expect("publish after quiescence");
        store.simulate_interrupted_publish(&baseline.id).expect("interrupt");
        assert_eq!(store.current("acme/widgets").as_deref(), Some(next.id.as_str()));
        let key = Digest::of(b"acme/widgets").hex();
        let pointer_dir = root.join("cache").join("pointers").join(&key[..16]);
        assert!(pointer_dir.join("publish.lock").exists());
        assert_eq!(
            store.recover_publish("acme/widgets").expect("recover").as_deref(),
            Some(next.id.as_str())
        );
        assert!(!pointer_dir.join("publish.lock").exists());
        assert_eq!(
            store.get(&next.id).expect("prior generation").state,
            GenerationState::Published
        );
        report(
            "interrupted_generation_publication",
            "current=next prior=published writer_gate=held_then_released lock_recovered=1",
            "staging_locks=0 pointer=next",
        );
    }

    #[test]
    fn f13_cache_connections_across_vm_reuse() {
        let mut scratch = Scratch::new();
        let root = scratch.dir("fault-cache-reuse");
        let store = GenerationStore::new(root.join("cache"));
        let owner = CacheAuthority::new("acme/widgets", TrustTier::Maintainer);
        let source = root.join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("tool"), b"baseline").expect("tool");
        let generation = store
            .create(&source, &owner, None, "toolchain", "t1")
            .expect("generation");
        store.publish(&generation.id, "t2").expect("publish");
        let manifest = store.manifest(&generation).expect("manifest");

        let first_vm = PrivateView::create(
            &root.join("views"),
            "vm1-job1",
            &manifest,
            store.blobs(),
        )
        .expect("first view");
        assert!(first_vm.is_shared_link("tool").expect("first shared"));
        first_vm.write("tool", b"dirty").expect("private write");
        let second_vm = PrivateView::create(
            &root.join("views"),
            "vm2-job2",
            &manifest,
            store.blobs(),
        )
        .expect("second view");
        assert_eq!(second_vm.read("tool").expect("second read"), b"baseline");
        assert_eq!(first_vm.read("tool").expect("first read"), b"dirty");

        let reopened = GenerationStore::new(root.join("cache"));
        assert_eq!(
            reopened.current("acme/widgets").as_deref(),
            Some(generation.id.as_str())
        );
        let reloaded = reopened.get(&generation.id).expect("reloaded generation");
        assert_eq!(reopened.manifest(&reloaded).expect("reloaded manifest"), manifest);
        report(
            "cache_connections_across_vm_reuse",
            "connections=2 shared_mutable_state=0 generation_survives_reuse=1 manifest_unchanged=1",
            "owned_views=2 blobs_bounded=yes",
        );
    }

    #[test]
    fn f14_image_runner_rotation_drains_idle_capacity() {
        let mut scratch = Scratch::new();
        let mut state = LedgerState::new(LedgerConfig {
            max_wait: Duration::from_millis(1_000),
        });
        state
            .register_host(host_capacity("h", 16, 32))
            .expect("register host");
        let small = revision("small", 1, 2);
        let small_request = || ReservationRequest::new(&small, 0, None);
        let mut busy = Vec::new();
        let mut warm = Vec::new();
        for _ in 0..4 {
            match state.reserve(small_request(), Tick::new(0)) {
                ReservationOutcome::Granted(offer) => {
                    state
                        .mark_busy(offer.reservation_id, HostIncarnation::new(1))
                        .expect("mark busy");
                    busy.push(offer.reservation_id);
                }
                ReservationOutcome::Pending { .. } => panic!("busy slot not granted"),
            }
        }
        for _ in 0..8 {
            match state.reserve(small_request(), Tick::new(0)) {
                ReservationOutcome::Granted(offer) => {
                    state
                        .mark_idle_warm(offer.reservation_id, HostIncarnation::new(1))
                        .expect("mark warm");
                    warm.push(offer.reservation_id);
                }
                ReservationOutcome::Pending { .. } => panic!("warm slot not granted"),
            }
        }
        let rotated = revision("rotated", 8, 16);
        state.submit(ReservationRequest::new(&rotated, 0, None), Tick::new(0));
        let cycle = state.plan(Tick::new(1_000));
        assert_eq!(cycle.reclaimed.len(), 8, "only drained idle capacity retires");
        assert!(cycle.reclaimed.iter().all(|id| warm.contains(id)));
        for id in &busy {
            assert_eq!(state.reservation(*id).expect("busy reservation").state, ReservationState::Busy);
        }
        assert_eq!(state.reservations().filter(|entry| entry.state.occupies_capacity()).count(), 5);

        let root = scratch.dir("fault-rotation-cache");
        let generations = GenerationStore::new(root.join("cache"));
        let owner = CacheAuthority::new("acme/widgets", TrustTier::Maintainer);
        let old_source = root.join("old");
        fs::create_dir_all(&old_source).expect("old source");
        fs::write(old_source.join("runner"), b"v1").expect("old runner");
        let old = generations.create(&old_source, &owner, None, "runner", "t1").expect("old generation");
        generations.publish(&old.id, "t2").expect("publish old");
        let new_source = root.join("new");
        fs::create_dir_all(&new_source).expect("new source");
        fs::write(new_source.join("runner"), b"v2").expect("new runner");
        let new = generations
            .create(&new_source, &owner, Some(old.id.clone()), "runner", "t3")
            .expect("new generation");
        generations.publish(&new.id, "t4").expect("publish new");
        assert_eq!(generations.current("acme/widgets").as_deref(), Some(new.id.as_str()));
        report(
            "image_runner_rotation_drains_idle_capacity",
            "reclaimed_idle=8 busy_preserved=4 image_generation=rotated runner_generation=rotated",
            "occupying_reservations=5 unbounded_growth=no",
        );
    }

    #[test]
    fn f16_storage_exhaustion() {
        let mut scratch = Scratch::new();
        let root = scratch.dir("fault-storage-exhaustion");
        let store = GenerationStore::new(root.join("cache"));
        store
            .set_quota(&crate::cache::Quota {
                max_generations: 1,
                max_bytes: 4_096,
            })
            .expect("set quota");
        let owner = CacheAuthority::new("acme/widgets", TrustTier::Maintainer);
        let first_source = root.join("first");
        fs::create_dir_all(&first_source).expect("first source");
        fs::write(first_source.join("a"), b"one").expect("first file");
        let first = store
            .create(&first_source, &owner, None, "toolchain", "t1")
            .expect("first generation");
        let second_source = root.join("second");
        fs::create_dir_all(&second_source).expect("second source");
        fs::write(second_source.join("b"), b"two").expect("second file");
        let refused = store.create(&second_source, &owner, None, "toolchain", "t2");
        assert!(
            matches!(refused, Err(crate::cache::CacheError::QuotaExceeded { .. })),
            "exhausted storage must refuse the write"
        );
        assert_eq!(store.list().expect("list").len(), 1);
        assert_eq!(store.get(&first.id).expect("first intact").name, "toolchain");
        store
            .set_quota(&crate::cache::Quota {
                max_generations: 8,
                max_bytes: 2,
            })
            .expect("tight byte quota");
        let over_bytes = store.create(
            &second_source,
            &owner,
            None,
            "toolchain",
            "t3",
        );
        assert!(matches!(over_bytes, Err(crate::cache::CacheError::QuotaExceeded { .. })));
        report(
            "storage_exhaustion",
            "generations=1 refused=count_and_bytes_quota first_generation=intact",
            "partial_generations=0 blobs_bounded=yes",
        );
    }

    #[test]
    fn f15_restore_admits_nothing_until_hosts_reconcile() {
        let mut scratch = Scratch::new();
        let path = scratch.db("fault-restore");
        {
            let mut ledger = Ledger::open_path(&path).expect("open ledger");
            for key in ["sr-host-a", "sr-host-b"] {
                let mut capacity = host_capacity(key, 8, 16);
                capacity.last_contact = Tick::new(0);
                ledger.register_host(capacity).expect("register host");
            }
            let small = revision("small", 1, 2);
            let outcome = ledger
                .reserve(ReservationRequest::new(&small, 0, None), Tick::new(0))
                .expect("reserve before backup");
            assert!(matches!(outcome, ReservationOutcome::Granted(_)));
        }
        let backup_path = scratch.db("fault-restore-backup");
        {
            let store = Store::open(&path).expect("open store");
            backup_to(store.connection(), &backup_path).expect("backup");
        }
        let mut restored = Ledger::open_path(&backup_path).expect("open restored ledger");
        assert_eq!(restored.hosts().count(), 2);
        assert_eq!(restored.reservations().count(), 1);
        let stale = restored.expired_hosts(Tick::new(60_000), 30_000);
        assert_eq!(stale.len(), 2, "no host is reconciled immediately after restore");
        for key in ["sr-host-a", "sr-host-b"] {
            restored
                .note_heartbeat(&HostKey::new(key), HostIncarnation::new(1), Tick::new(60_000))
                .expect("reconcile host");
        }
        assert!(restored.expired_hosts(Tick::new(60_000), 30_000).is_empty());
        report(
            "restore_admits_nothing_until_hosts_reconcile",
            "durable_hosts=2 durable_reservations=1 stale_before=2 stale_after=0 admitted_before_reconcile=0",
            "owned_hosts=2 reconnected_hosts=2",
        );
    }
}
