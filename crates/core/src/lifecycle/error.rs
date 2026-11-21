use std::fmt;

use crate::lifecycle::state::ExecutionState;
use crate::ledger::LedgerError;
use crate::store::StoreError;

pub type Result<T> = std::result::Result<T, LifecycleError>;

#[derive(Debug)]
pub enum LifecycleError {
    Store(StoreError),
    Ledger(LedgerError),
    InvalidTransition {
        from: ExecutionState,
        to: ExecutionState,
    },
    CorruptDecision(String),
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(f, "lifecycle store: {error}"),
            Self::Ledger(error) => write!(f, "lifecycle ledger: {error}"),
            Self::InvalidTransition { from, to } => {
                write!(f, "invalid lifecycle transition {from} -> {to}")
            }
            Self::CorruptDecision(value) => write!(f, "corrupt lifecycle decision: {value}"),
        }
    }
}

impl std::error::Error for LifecycleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Ledger(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StoreError> for LifecycleError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<LedgerError> for LifecycleError {
    fn from(error: LedgerError) -> Self {
        Self::Ledger(error)
    }
}
