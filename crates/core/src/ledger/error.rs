use std::fmt;

use crate::domain::Shape;
use crate::ledger::types::{CacheZone, CapacityOffer, HostKey, ReservationId, ReservationState};
use crate::store::StoreError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnschedulableReason {
    NoHostsRegistered,
    ArchitectureMismatch { required: &'static str },
    HostIneligible { host_key: HostKey, reason: String },
    ShapeExceedsHostCapacity {
        shape: Shape,
        max_vcpu: u32,
        max_memory_gib: u32,
    },
    InsufficientCapacity { host_key: HostKey, shape: Shape },
    NoReclaimableCapacity { host_key: HostKey, shape: Shape },
}

impl fmt::Display for UnschedulableReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHostsRegistered => write!(f, "no hosts are registered"),
            Self::ArchitectureMismatch { required } => {
                write!(f, "no eligible host has architecture {required}")
            }
            Self::HostIneligible { host_key, reason } => {
                write!(f, "host {host_key} is ineligible: {reason}")
            }
            Self::ShapeExceedsHostCapacity {
                shape,
                max_vcpu,
                max_memory_gib,
            } => write!(
                f,
                "shape {}c{}g exceeds the largest host ({max_vcpu}c/{max_memory_gib}g)",
                shape.vcpu().get(),
                shape.memory_gib()
            ),
            Self::InsufficientCapacity { host_key, shape } => write!(
                f,
                "host {host_key} lacks capacity for {}c{}g",
                shape.vcpu().get(),
                shape.memory_gib()
            ),
            Self::NoReclaimableCapacity { host_key, shape } => write!(
                f,
                "host {host_key} has only busy capacity; cannot reclaim for {}c{}g",
                shape.vcpu().get(),
                shape.memory_gib()
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingReason {
    Queued,
    BehindBarrier { barrier_sequence: u64 },
    AwaitingCacheLocality { zone: CacheZone },
}

impl fmt::Display for PendingReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queued => write!(f, "queued for scheduling"),
            Self::BehindBarrier { barrier_sequence } => {
                write!(f, "held behind reservation barrier at {barrier_sequence}")
            }
            Self::AwaitingCacheLocality { zone } => {
                write!(f, "waiting for cache locality on {zone}")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingStatus {
    Granted { offer: CapacityOffer },
    Waiting {
        reason: PendingReason,
        age_ms: u64,
        due: bool,
    },
    Unschedulable {
        reason: UnschedulableReason,
        age_ms: u64,
        due: bool,
    },
}

impl PendingStatus {
    pub fn is_granted(&self) -> bool {
        matches!(self, Self::Granted { .. })
    }
}

#[derive(Debug, PartialEq)]
pub enum LedgerError {
    StaleHostIncarnation {
        host_key: HostKey,
        expected: u64,
        found: u64,
    },
    ReservationIncarnationMismatch {
        reservation_id: ReservationId,
        expected: u64,
        found: u64,
    },
    UnknownHost(HostKey),
    UnknownReservation(ReservationId),
    InvalidStateTransition {
        reservation_id: ReservationId,
        from: ReservationState,
        to: ReservationState,
    },
    Store(StoreError),
}

impl From<StoreError> for LedgerError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleHostIncarnation {
                host_key,
                expected,
                found,
            } => write!(
                f,
                "stale incarnation {found} for host {host_key}; current is {expected}"
            ),
            Self::ReservationIncarnationMismatch {
                reservation_id,
                expected,
                found,
            } => write!(
                f,
                "incarnation {found} cannot act on reservation {}; owner is {expected}",
                reservation_id.get()
            ),
            Self::UnknownHost(host_key) => write!(f, "unknown host {host_key}"),
            Self::UnknownReservation(reservation_id) => {
                write!(f, "unknown reservation {}", reservation_id.get())
            }
            Self::InvalidStateTransition {
                reservation_id,
                from,
                to,
            } => write!(
                f,
                "reservation {} cannot move from {from:?} to {to:?}",
                reservation_id.get()
            ),
            Self::Store(error) => write!(f, "ledger store error: {error}"),
        }
    }
}

impl std::error::Error for LedgerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}
