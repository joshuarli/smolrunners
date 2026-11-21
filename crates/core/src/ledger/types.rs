use std::fmt;
use std::time::Duration;

use crate::domain::{Arch, ProfileRevision, ProfileRevisionId, Shape};

pub const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tick(u64);

impl Tick {
    pub const fn new(milliseconds: u64) -> Self {
        Self(milliseconds)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn elapsed_since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostKey(String);

impl HostKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HostKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostIncarnation(u64);

impl HostIncarnation {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReservationId(u64);

impl ReservationId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheZone(String);

impl CacheZone {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CacheZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VmmOverhead {
    pub measured_memory_mib: u32,
    pub measured_vcpu_milli: u32,
}

impl VmmOverhead {
    pub const fn new(measured_memory_mib: u32, measured_vcpu_milli: u32) -> Self {
        Self {
            measured_memory_mib,
            measured_vcpu_milli,
        }
    }

    pub const fn zero() -> Self {
        Self::new(0, 0)
    }
}

impl Default for VmmOverhead {
    fn default() -> Self {
        Self::new(128, 50)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerConfig {
    pub max_wait: Duration,
}

impl LedgerConfig {
    pub fn max_wait_ms(&self) -> u64 {
        u64::try_from(self.max_wait.as_millis()).unwrap_or(u64::MAX)
    }
}

impl Default for LedgerConfig {
    fn default() -> Self {
        Self {
            max_wait: DEFAULT_MAX_WAIT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostAdmission {
    Eligible,
    Ineligible { reason: String },
}

impl HostAdmission {
    pub fn is_eligible(&self) -> bool {
        matches!(self, Self::Eligible)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostCapacity {
    pub host_key: HostKey,
    pub incarnation: HostIncarnation,
    pub hostname: String,
    pub arch: Arch,
    pub total_vcpu: u32,
    pub total_memory_gib: u32,
    pub total_disk_bytes: u64,
    pub headroom_vcpu: u32,
    pub headroom_memory_gib: u32,
    pub headroom_disk_bytes: u64,
    pub vmm_overhead: VmmOverhead,
    pub admission: HostAdmission,
    pub cache_zone: Option<CacheZone>,
    pub last_contact: Tick,
}

impl HostCapacity {
    pub fn reservable_vcpu_milli(&self) -> u64 {
        u64::from(self.total_vcpu.saturating_sub(self.headroom_vcpu)).saturating_mul(1000)
    }

    pub fn reservable_memory_mib(&self) -> u64 {
        u64::from(self.total_memory_gib.saturating_sub(self.headroom_memory_gib))
            .saturating_mul(1024)
    }

    pub fn reservable_disk_bytes(&self) -> u64 {
        self.total_disk_bytes.saturating_sub(self.headroom_disk_bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DemandCost {
    pub vcpu_milli: u64,
    pub memory_mib: u64,
    pub disk_bytes: u64,
}

pub(crate) fn demand_cost(shape: Shape, overhead: VmmOverhead, disk_bytes: u64) -> DemandCost {
    DemandCost {
        vcpu_milli: u64::from(shape.vcpu().get()).saturating_mul(1000) + u64::from(overhead.measured_vcpu_milli),
        memory_mib: u64::from(shape.memory_gib()).saturating_mul(1024) + u64::from(overhead.measured_memory_mib),
        disk_bytes,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReservationRequest {
    pub revision_id: ProfileRevisionId,
    pub arch: Arch,
    pub shape: Shape,
    pub disk_bytes: u64,
    pub preferred_cache_zone: Option<CacheZone>,
}

impl ReservationRequest {
    pub fn new(
        revision: &ProfileRevision,
        disk_bytes: u64,
        preferred_cache_zone: Option<CacheZone>,
    ) -> Self {
        Self {
            revision_id: revision.id(),
            arch: revision.profile().target().arch(),
            shape: revision.profile().shape(),
            disk_bytes,
            preferred_cache_zone,
        }
    }

    pub fn cost(&self, overhead: VmmOverhead) -> DemandCost {
        demand_cost(self.shape, overhead, self.disk_bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReservationState {
    Pending,
    Reserved,
    IdleWarm,
    Busy,
    Released,
}

impl ReservationState {
    pub fn occupies_capacity(self) -> bool {
        matches!(self, Self::Reserved | Self::IdleWarm | Self::Busy)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Reserved => "reserved",
            Self::IdleWarm => "idle_warm",
            Self::Busy => "busy",
            Self::Released => "released",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "reserved" => Some(Self::Reserved),
            "idle_warm" => Some(Self::IdleWarm),
            "busy" => Some(Self::Busy),
            "released" => Some(Self::Released),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    pub id: ReservationId,
    pub sequence: u64,
    pub enqueued_at: Tick,
    pub state: ReservationState,
    pub request: ReservationRequest,
    pub host: Option<HostKey>,
    pub incarnation: HostIncarnation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapacityOffer {
    pub reservation_id: ReservationId,
    pub host_key: HostKey,
    pub incarnation: HostIncarnation,
    pub revision_id: ProfileRevisionId,
    pub shape: Shape,
    pub disk_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Usage {
    pub vcpu_milli: u64,
    pub memory_mib: u64,
    pub disk_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScaleSetOffer {
    pub revision_id: ProfileRevisionId,
    pub slots: u32,
}
