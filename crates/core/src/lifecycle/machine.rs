use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::lifecycle::error::{LifecycleError, Result};
use crate::lifecycle::state::{Decision, ExecutionState, ObservationKind};
use crate::lifecycle::store;
use crate::proto::Frame;
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    ReserveCapacity,
    CreateGuest,
    StartSampling,
    DeliverJit,
    RegisterRunner,
    AcquireJobs,
    JoinIdentities,
    FlushDeregister,
    DestroyResources,
    ReleaseCapacity,
}

impl Effect {
    pub const ALL: [Effect; 10] = [
        Self::ReserveCapacity,
        Self::CreateGuest,
        Self::StartSampling,
        Self::DeliverJit,
        Self::RegisterRunner,
        Self::AcquireJobs,
        Self::JoinIdentities,
        Self::FlushDeregister,
        Self::DestroyResources,
        Self::ReleaseCapacity,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReserveCapacity => "reserve_capacity",
            Self::CreateGuest => "create_guest",
            Self::StartSampling => "start_sampling",
            Self::DeliverJit => "deliver_jit",
            Self::RegisterRunner => "register_runner",
            Self::AcquireJobs => "acquire_jobs",
            Self::JoinIdentities => "join_identities",
            Self::FlushDeregister => "flush_deregister",
            Self::DestroyResources => "destroy_resources",
            Self::ReleaseCapacity => "release_capacity",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KillPoint {
    Before(Effect),
    After(Effect),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectResult {
    Committed,
    Uncertain,
}

pub trait EffectSink {
    fn perform(&mut self, effect: Effect) -> Result<EffectResult>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DecisionRecord {
    decision: Decision,
    recorded: bool,
    state: Option<String>,
    effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submitted {
    pub ack: Frame,
    pub decision: Decision,
    pub state: Option<ExecutionState>,
    pub effects: Vec<Effect>,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    Completed(Submitted),
    Killed,
}

struct Plan {
    decision: Decision,
    effects: Vec<Effect>,
    state: Option<ExecutionState>,
    execution_key: String,
    job_id: Option<String>,
    acquisition: Option<(Vec<i64>, Vec<i64>)>,
}

pub struct Lifecycle {
    store: Store,
}

impl Lifecycle {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let store = store::open(path)?;
        Ok(Self { store })
    }

    pub fn from_store(store: Store) -> Result<Self> {
        store::initialize(store.connection())?;
        Ok(Self { store })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn submit(
        &mut self,
        frame: &Frame,
        now: &str,
        sink: &mut dyn EffectSink,
    ) -> Result<Submitted> {
        match self.submit_with_kill(frame, now, sink, None)? {
            SubmitOutcome::Completed(submitted) => Ok(submitted),
            SubmitOutcome::Killed => unreachable!("no kill point requested"),
        }
    }

    pub fn submit_with_kill(
        &mut self,
        frame: &Frame,
        now: &str,
        sink: &mut dyn EffectSink,
        kill: Option<KillPoint>,
    ) -> Result<SubmitOutcome> {
        let event_id = frame.id().to_owned();
        let payload = serde_json::to_string(frame).map_err(|error| {
            LifecycleError::CorruptDecision(format!("encode frame: {error}"))
        })?;
        let is_new = store::insert_event(
            self.store.connection(),
            &event_id,
            frame.kind(),
            &payload,
            now,
        )?;
        if !is_new {
            return Ok(SubmitOutcome::Completed(self.replay(&event_id, now)?));
        }

        store::mark_processing(self.store.connection(), &event_id)?;
        let plan = Self::plan(frame);
        self.process(event_id, plan, now, sink, kill)
    }

    pub fn submit_effects(
        &mut self,
        event_id: &str,
        effects: &[Effect],
        state: Option<ExecutionState>,
        now: &str,
        sink: &mut dyn EffectSink,
        kill: Option<KillPoint>,
    ) -> Result<SubmitOutcome> {
        let is_new = store::insert_event(
            self.store.connection(),
            event_id,
            "host_progress",
            "{}",
            now,
        )?;
        if !is_new {
            return Ok(SubmitOutcome::Completed(self.replay(event_id, now)?));
        }
        store::mark_processing(self.store.connection(), event_id)?;
        let plan = Plan {
            decision: Decision::Accept,
            effects: effects.to_vec(),
            state,
            execution_key: event_id.to_owned(),
            job_id: None,
            acquisition: None,
        };
        self.process(event_id.to_owned(), plan, now, sink, kill)
    }

    fn process(
        &mut self,
        event_id: String,
        plan: Plan,
        now: &str,
        sink: &mut dyn EffectSink,
        kill: Option<KillPoint>,
    ) -> Result<SubmitOutcome> {
        let mut performed = Vec::new();
        for effect in &plan.effects {
            if kill == Some(KillPoint::Before(*effect)) {
                return Ok(SubmitOutcome::Killed);
            }
            let result = sink.perform(*effect)?;
            performed.push(*effect);
            if result == EffectResult::Uncertain {
                return Ok(SubmitOutcome::Completed(self.finish_uncertain(
                    &event_id,
                    &plan.execution_key,
                    plan.job_id.as_deref(),
                    &performed,
                    now,
                )?));
            }
            if kill == Some(KillPoint::After(*effect)) {
                return Ok(SubmitOutcome::Killed);
            }
        }

        if plan.decision == Decision::Reject {
            let detail = serde_json::json!({
                "event_id": event_id,
                "reason": "rejected",
            })
            .to_string();
            store::insert_observation(
                self.store.connection(),
                &event_id,
                ObservationKind::Failed.as_str(),
                &detail,
                now,
            )?;
        }

        if let Some((request_ids, acquired_ids)) = &plan.acquisition {
            let request_json = serde_json::to_string(request_ids).expect("serialize request ids");
            let acquired_json = serde_json::to_string(acquired_ids).expect("serialize acquired ids");
            store::insert_acquisition(
                self.store.connection(),
                &event_id,
                &request_json,
                &acquired_json,
                performed.len() as i64,
                now,
            )?;
        }
        if let Some(state) = plan.state {
            store::upsert_execution(
                self.store.connection(),
                &plan.execution_key,
                plan.job_id.as_deref(),
                state.as_str(),
                None,
                now,
            )?;
        }

        self.finish(event_id, plan.decision, plan.state, performed, now)
            .map(SubmitOutcome::Completed)
    }

    fn replay(&mut self, event_id: &str, now: &str) -> Result<Submitted> {
        let event = store::load_event(self.store.connection(), event_id)?
            .expect("an inserted event is present");
        if event.state == store::INBOX_DONE {
            let record = Self::decode_decision(event.decision_json.as_deref())?;
            return Ok(Submitted {
                ack: Self::ack(event_id, record.decision, record.recorded),
                decision: record.decision,
                state: record
                    .state
                    .as_deref()
                    .and_then(ExecutionState::parse),
                effects: Vec::new(),
                replayed: true,
            });
        }
        self.finish_uncertain(event_id, event_id, None, &[], now)
    }

    fn finish_uncertain(
        &mut self,
        event_id: &str,
        execution_key: &str,
        job_id: Option<&str>,
        effects: &[Effect],
        now: &str,
    ) -> Result<Submitted> {
        let detail = serde_json::json!({
            "event_id": event_id,
            "reason": "outcome_unknown",
            "effects": effects.iter().map(|effect| effect.as_str()).collect::<Vec<_>>(),
        })
        .to_string();
        store::insert_observation(
            self.store.connection(),
            event_id,
            ObservationKind::Uncertain.as_str(),
            &detail,
            now,
        )?;
        store::upsert_execution(
            self.store.connection(),
            execution_key,
            job_id,
            ExecutionState::Uncertain.as_str(),
            None,
            now,
        )?;
        self.finish(
            event_id.to_owned(),
            Decision::Uncertain,
            Some(ExecutionState::Uncertain),
            effects.to_vec(),
            now,
        )
    }

    fn finish(
        &mut self,
        event_id: String,
        decision: Decision,
        state: Option<ExecutionState>,
        effects: Vec<Effect>,
        now: &str,
    ) -> Result<Submitted> {
        let record = DecisionRecord {
            decision,
            recorded: true,
            state: state.map(ExecutionState::as_str).map(str::to_owned),
            effects: effects.clone(),
        };
        let decision_json = serde_json::to_string(&record)
            .map_err(|error| LifecycleError::CorruptDecision(error.to_string()))?;
        store::mark_done(self.store.connection(), &event_id, &decision_json, now)?;
        Ok(Submitted {
            ack: Self::ack(&event_id, decision, true),
            decision,
            state,
            effects,
            replayed: false,
        })
    }

    pub fn recover(&mut self, now: &str) -> Result<Vec<String>> {
        let ids = store::processing_event_ids(self.store.connection())?;
        for event_id in &ids {
            let detail = serde_json::json!({
                "event_id": event_id,
                "reason": "coordinator_interrupted_before_ack",
            })
            .to_string();
            store::insert_observation(
                self.store.connection(),
                event_id,
                ObservationKind::Uncertain.as_str(),
                &detail,
                now,
            )?;
            store::upsert_execution(
                self.store.connection(),
                event_id,
                None,
                ExecutionState::Uncertain.as_str(),
                None,
                now,
            )?;
            let record = DecisionRecord {
                decision: Decision::Uncertain,
                recorded: true,
                state: Some(ExecutionState::Uncertain.as_str().to_owned()),
                effects: Vec::new(),
            };
            let decision_json = serde_json::to_string(&record)
                .map_err(|error| LifecycleError::CorruptDecision(error.to_string()))?;
            store::mark_done(self.store.connection(), event_id, &decision_json, now)?;
        }
        Ok(ids)
    }

    pub fn transition(
        &mut self,
        execution_key: &str,
        next: ExecutionState,
        now: &str,
    ) -> Result<()> {
        let current_text = store::load_execution_state(self.store.connection(), execution_key)?
            .ok_or(LifecycleError::InvalidTransition {
                from: ExecutionState::Reserved,
                to: next,
            })?;
        let current = ExecutionState::parse(&current_text).ok_or_else(|| {
            LifecycleError::CorruptDecision(format!("unknown state {current_text}"))
        })?;
        if !current.may_transition_to(next) {
            return Err(LifecycleError::InvalidTransition {
                from: current,
                to: next,
            });
        }
        Ok(store::upsert_execution(
            self.store.connection(),
            execution_key,
            None,
            next.as_str(),
            None,
            now,
        )?)
    }

    pub fn state_of(&self, execution_key: &str) -> Result<Option<ExecutionState>> {
        Ok(store::load_execution_state(self.store.connection(), execution_key)?
            .and_then(|text| ExecutionState::parse(&text)))
    }

    pub fn acquisition_exists(&self, event_id: &str) -> Result<bool> {
        Ok(store::load_acquisition(self.store.connection(), event_id)?.is_some())
    }

    pub fn acquisition_count(&self) -> Result<i64> {
        Ok(store::acquisition_count(self.store.connection())?)
    }

    pub fn observation(&self, event_id: &str) -> Result<Option<store::ObservationRow>> {
        Ok(store::load_observation(self.store.connection(), event_id)?)
    }

    fn ack(event_id: &str, decision: Decision, recorded: bool) -> Frame {
        Frame::Ack {
            schema_version: crate::proto::SCHEMA_VERSION,
            id: format!("ack-{event_id}"),
            event_id: event_id.to_owned(),
            decision: decision.as_str().to_owned(),
            recorded,
        }
    }

    fn decode_decision(text: Option<&str>) -> Result<DecisionRecord> {
        let text = text.ok_or_else(|| LifecycleError::CorruptDecision("missing decision".to_owned()))?;
        serde_json::from_str(text)
            .map_err(|error| LifecycleError::CorruptDecision(error.to_string()))
    }

    fn plan(frame: &Frame) -> Plan {
        let rejected = frame_reason(frame)
            .map(|reason| !reason.is_empty())
            .unwrap_or(false);
        let decision = if rejected {
            Decision::Reject
        } else {
            Decision::Accept
        };
        let job_id = frame_job_id(frame).map(str::to_owned);
        let execution_key = job_id
            .clone()
            .unwrap_or_else(|| frame.id().to_owned());
        let mut effects = Vec::new();
        let mut state = None;
        let mut acquisition = None;

        if !rejected {
            match frame {
                Frame::JobAvailable { .. } => {
                    effects.push(Effect::ReserveCapacity);
                    state = Some(ExecutionState::Reserved);
                }
                Frame::AcquireOutcome {
                    request_ids,
                    acquired_ids,
                    ..
                } => {
                    effects.push(Effect::AcquireJobs);
                    acquisition = Some((
                        request_ids.iter().map(|id| id.get()).collect(),
                        acquired_ids.iter().map(|id| id.get()).collect(),
                    ));
                }
                Frame::JobAssigned { .. } => {
                    effects.push(Effect::JoinIdentities);
                    state = Some(ExecutionState::Busy);
                }
                Frame::JobCompleted { .. } => {
                    effects.extend([
                        Effect::FlushDeregister,
                        Effect::DestroyResources,
                        Effect::ReleaseCapacity,
                    ]);
                    state = Some(ExecutionState::Removed);
                }
                Frame::VmDelete { .. } => {
                    effects.push(Effect::DestroyResources);
                    state = Some(ExecutionState::Removed);
                }
                Frame::JobStarted { .. } => {
                    state = Some(ExecutionState::Busy);
                }
                Frame::Run { .. }
                | Frame::Capacity { .. }
                | Frame::Session { .. }
                | Frame::Ack { .. }
                | Frame::JitRequest { .. }
                | Frame::RemoveRunner { .. }
                | Frame::CommandResult { .. } => {}
            }
        } else {
            state = Some(ExecutionState::Failed);
        }

        Plan {
            decision,
            effects,
            state,
            execution_key,
            job_id,
            acquisition,
        }
    }
}

fn frame_reason(frame: &Frame) -> Option<&str> {
    match frame {
        Frame::JobAvailable { reason, .. }
        | Frame::JobAssigned { reason, .. }
        | Frame::AcquireOutcome { reason, .. }
        | Frame::Session { reason, .. } => reason.as_deref(),
        _ => None,
    }
}

fn frame_job_id(frame: &Frame) -> Option<&str> {
    match frame {
        Frame::JobAvailable { job_id, .. }
        | Frame::JobAssigned { job_id, .. }
        | Frame::JobStarted { job_id, .. }
        | Frame::JobCompleted { job_id, .. }
        | Frame::VmDelete { job_id, .. } => Some(job_id.as_str()),
        _ => None,
    }
}
