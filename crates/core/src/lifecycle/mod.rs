mod error;
mod harness;
mod machine;
mod state;
mod store;

pub use error::{LifecycleError, Result};
pub use harness::{
    CrashHarness, CrashReport, LedgerSink, RecordingSink,
};
pub use machine::{
    Effect, EffectResult, EffectSink, KillPoint, Lifecycle, SubmitOutcome, Submitted,
};
pub use state::{Decision, ExecutionState, ObservationKind};
pub use store::{
    AcquisitionRow, InboxEvent, ObservationRow, INBOX_DONE, INBOX_PROCESSING, INBOX_RECEIVED,
};

#[cfg(test)]
mod tests;
