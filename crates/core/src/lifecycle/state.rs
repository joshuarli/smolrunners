use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Reserved,
    Provisioning,
    GuestReady,
    Registering,
    Idle,
    Busy,
    Finalizing,
    Terminating,
    Removed,
    Failed,
    Uncertain,
}

impl ExecutionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Provisioning => "provisioning",
            Self::GuestReady => "guest_ready",
            Self::Registering => "registering",
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::Finalizing => "finalizing",
            Self::Terminating => "terminating",
            Self::Removed => "removed",
            Self::Failed => "failed",
            Self::Uncertain => "uncertain",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "reserved" => Some(Self::Reserved),
            "provisioning" => Some(Self::Provisioning),
            "guest_ready" => Some(Self::GuestReady),
            "registering" => Some(Self::Registering),
            "idle" => Some(Self::Idle),
            "busy" => Some(Self::Busy),
            "finalizing" => Some(Self::Finalizing),
            "terminating" => Some(Self::Terminating),
            "removed" => Some(Self::Removed),
            "failed" => Some(Self::Failed),
            "uncertain" => Some(Self::Uncertain),
            _ => None,
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Removed | Self::Failed)
    }

    pub const fn may_transition_to(self, next: Self) -> bool {
        match self {
            Self::Reserved => matches!(next, Self::Provisioning | Self::Failed | Self::Uncertain),
            Self::Provisioning => matches!(next, Self::GuestReady | Self::Failed | Self::Uncertain),
            Self::GuestReady => matches!(next, Self::Registering | Self::Failed | Self::Uncertain),
            Self::Registering => matches!(next, Self::Idle | Self::Failed | Self::Uncertain),
            Self::Idle => matches!(next, Self::Busy | Self::Terminating | Self::Failed | Self::Uncertain),
            Self::Busy => matches!(next, Self::Finalizing | Self::Failed | Self::Uncertain),
            Self::Finalizing => matches!(next, Self::Terminating | Self::Failed | Self::Uncertain),
            Self::Terminating => matches!(next, Self::Removed | Self::Failed | Self::Uncertain),
            Self::Removed | Self::Failed | Self::Uncertain => false,
        }
    }
}

impl fmt::Display for ExecutionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObservationKind {
    Failed,
    Uncertain,
}

impl ObservationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Uncertain => "uncertain",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "failed" => Some(Self::Failed),
            "uncertain" => Some(Self::Uncertain),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Accept,
    Reject,
    Uncertain,
}

impl Decision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
            Self::Uncertain => "uncertain",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "accept" => Some(Self::Accept),
            "reject" => Some(Self::Reject),
            "uncertain" => Some(Self::Uncertain),
            _ => None,
        }
    }
}
