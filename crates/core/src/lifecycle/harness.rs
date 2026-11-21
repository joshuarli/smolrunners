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
