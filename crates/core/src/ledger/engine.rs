use std::collections::BTreeMap;

use crate::domain::ProfileRevision;
use crate::ledger::error::{LedgerError, PendingReason, PendingStatus, UnschedulableReason};
use crate::ledger::types::{
    CapacityOffer, DemandCost, HostCapacity, HostIncarnation, HostKey, LedgerConfig, Reservation,
    ReservationId, ReservationRequest, ReservationState, ScaleSetOffer, Tick, Usage, VmmOverhead,
};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Cycle {
    pub offers: Vec<CapacityOffer>,
    pub reclaimed: Vec<ReservationId>,
    pub barrier: Option<u64>,
    pub statuses: Vec<(ReservationId, PendingStatus)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReservationOutcome {
    Granted(CapacityOffer),
    Pending {
        id: ReservationId,
        status: PendingStatus,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerState {
    config: LedgerConfig,
    hosts: BTreeMap<HostKey, HostCapacity>,
    reservations: BTreeMap<ReservationId, Reservation>,
    statuses: BTreeMap<ReservationId, PendingStatus>,
    next_id: u64,
    next_sequence: u64,
    last_barrier: Option<u64>,
}

impl LedgerState {
    pub fn new(config: LedgerConfig) -> Self {
        Self {
            config,
            hosts: BTreeMap::new(),
            reservations: BTreeMap::new(),
            statuses: BTreeMap::new(),
            next_id: 1,
            next_sequence: 1,
            last_barrier: None,
        }
    }

    pub fn config(&self) -> &LedgerConfig {
        &self.config
    }

    pub fn barrier(&self) -> Option<u64> {
        self.last_barrier
    }

    pub fn hosts(&self) -> impl Iterator<Item = &HostCapacity> {
        self.hosts.values()
    }

    pub fn host(&self, host_key: &HostKey) -> Option<&HostCapacity> {
        self.hosts.get(host_key)
    }

    pub fn reservations(&self) -> impl Iterator<Item = &Reservation> {
        self.reservations.values()
    }

    pub fn reservation(&self, id: ReservationId) -> Option<&Reservation> {
        self.reservations.get(&id)
    }

    pub fn register_host(&mut self, capacity: HostCapacity) -> Result<(), LedgerError> {
        if let Some(current) = self.hosts.get(&capacity.host_key) {
            if capacity.incarnation < current.incarnation {
                return Err(LedgerError::StaleHostIncarnation {
                    host_key: capacity.host_key.clone(),
                    expected: current.incarnation.get(),
                    found: capacity.incarnation.get(),
                });
            }
        }
        self.hosts.insert(capacity.host_key.clone(), capacity);
        Ok(())
    }

    pub fn deregister_host(
        &mut self,
        host_key: &HostKey,
        incarnation: HostIncarnation,
    ) -> Result<(), LedgerError> {
        let current = self
            .hosts
            .get(host_key)
            .ok_or_else(|| LedgerError::UnknownHost(host_key.clone()))?;
        if current.incarnation != incarnation {
            return Err(LedgerError::StaleHostIncarnation {
                host_key: host_key.clone(),
                expected: current.incarnation.get(),
                found: incarnation.get(),
            });
        }
        self.hosts.remove(host_key);
        Ok(())
    }

    pub fn submit(&mut self, request: ReservationRequest, now: Tick) -> ReservationId {
        let id = ReservationId::new(self.next_id);
        self.next_id += 1;
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.reservations.insert(
            id,
            Reservation {
                id,
                sequence,
                enqueued_at: now,
                state: ReservationState::Pending,
                request,
                host: None,
                incarnation: HostIncarnation::new(0),
            },
        );
        self.statuses.insert(
            id,
            PendingStatus::Waiting {
                reason: PendingReason::Queued,
                age_ms: 0,
                due: false,
            },
        );
        id
    }

    pub fn reserve(&mut self, request: ReservationRequest, now: Tick) -> ReservationOutcome {
        let id = self.submit(request, now);
        self.plan(now);
        match self
            .status(id, now)
            .expect("a just-submitted reservation has a status")
        {
            PendingStatus::Granted { offer } => ReservationOutcome::Granted(offer),
            status => ReservationOutcome::Pending { id, status },
        }
    }

    pub fn status(&self, id: ReservationId, now: Tick) -> Option<PendingStatus> {
        let reservation = self.reservations.get(&id)?;
        if let Some(status) = self.statuses.get(&id) {
            return Some(status.clone());
        }
        if let Some(host_key) = &reservation.host {
            return Some(PendingStatus::Granted {
                offer: self.offer_for(reservation, host_key),
            });
        }
        let age_ms = now.elapsed_since(reservation.enqueued_at);
        Some(PendingStatus::Waiting {
            reason: PendingReason::Queued,
            age_ms,
            due: age_ms >= self.config.max_wait_ms(),
        })
    }

    pub fn usage(&self, host_key: &HostKey) -> Usage {
        self.compute_usage().get(host_key).copied().unwrap_or_default()
    }

    pub fn offers_for(&self, revision: &ProfileRevision) -> ScaleSetOffer {
        let request_shape = revision.profile().shape();
        let arch = revision.profile().target().arch();
        let mut slots = 0u32;
        let usage = self.compute_usage();
        for host in self.hosts.values() {
            if !host.admission.is_eligible() || host.arch != arch {
                continue;
            }
            let cost = crate::ledger::types::demand_cost(request_shape, host.vmm_overhead, 0);
            let used = &usage[&host.host_key];
            let by_cpu = host
                .reservable_vcpu_milli()
                .saturating_sub(used.vcpu_milli)
                .checked_div(cost.vcpu_milli)
                .unwrap_or(0);
            let by_memory = host
                .reservable_memory_mib()
                .saturating_sub(used.memory_mib)
                .checked_div(cost.memory_mib)
                .unwrap_or(0);
            let by_disk = host
                .reservable_disk_bytes()
                .saturating_sub(used.disk_bytes)
                .checked_div(cost.disk_bytes.max(1))
                .unwrap_or(u64::MAX);
            let available = by_cpu.min(by_memory).min(by_disk);
            slots = slots.saturating_add(u32::try_from(available).unwrap_or(u32::MAX));
        }
        ScaleSetOffer {
            revision_id: revision.id(),
            slots,
        }
    }

    pub fn note_heartbeat(
        &mut self,
        host_key: &HostKey,
        incarnation: HostIncarnation,
        now: Tick,
    ) -> Result<(), LedgerError> {
        let host = self
            .hosts
            .get_mut(host_key)
            .ok_or_else(|| LedgerError::UnknownHost(host_key.clone()))?;
        if host.incarnation != incarnation {
            return Err(LedgerError::StaleHostIncarnation {
                host_key: host_key.clone(),
                expected: host.incarnation.get(),
                found: incarnation.get(),
            });
        }
        host.last_contact = now;
        Ok(())
    }

    pub fn expired_hosts(&self, now: Tick, lease_ms: u64) -> Vec<HostKey> {
        self.hosts
            .values()
            .filter(|host| now.elapsed_since(host.last_contact) >= lease_ms)
            .map(|host| host.host_key.clone())
            .collect()
    }

    pub fn mark_busy(
        &mut self,
        id: ReservationId,
        incarnation: HostIncarnation,
    ) -> Result<(), LedgerError> {
        self.transition(
            id,
            incarnation,
            ReservationState::Busy,
            &[ReservationState::Reserved, ReservationState::IdleWarm],
        )
    }

    pub fn mark_idle_warm(
        &mut self,
        id: ReservationId,
        incarnation: HostIncarnation,
    ) -> Result<(), LedgerError> {
        self.transition(
            id,
            incarnation,
            ReservationState::IdleWarm,
            &[ReservationState::Reserved, ReservationState::Busy],
        )
    }

    pub fn release(
        &mut self,
        id: ReservationId,
        host_key: &HostKey,
        incarnation: HostIncarnation,
        _now: Tick,
    ) -> Result<(), LedgerError> {
        let current = self
            .hosts
            .get(host_key)
            .ok_or_else(|| LedgerError::UnknownHost(host_key.clone()))?;
        if current.incarnation != incarnation {
            return Err(LedgerError::StaleHostIncarnation {
                host_key: host_key.clone(),
                expected: current.incarnation.get(),
                found: incarnation.get(),
            });
        }
        let reservation = self
            .reservations
            .get(&id)
            .ok_or(LedgerError::UnknownReservation(id))?;
        if reservation.incarnation != incarnation {
            return Err(LedgerError::ReservationIncarnationMismatch {
                reservation_id: id,
                expected: reservation.incarnation.get(),
                found: incarnation.get(),
            });
        }
        if !reservation.state.occupies_capacity() {
            return Err(LedgerError::InvalidStateTransition {
                reservation_id: id,
                from: reservation.state,
                to: ReservationState::Released,
            });
        }
        let mut usage = self.compute_usage();
        self.release_internal(id, &mut usage);
        self.statuses.remove(&id);
        Ok(())
    }

    pub fn plan(&mut self, now: Tick) -> Cycle {
        let mut cycle = Cycle::default();
        let max_wait_ms = self.config.max_wait_ms();
        let mut usage = self.compute_usage();

        let mut pending: Vec<ReservationId> = self
            .reservations
            .values()
            .filter(|reservation| reservation.state == ReservationState::Pending)
            .map(|reservation| reservation.id)
            .collect();
        pending.sort_by_key(|id| self.reservations[id].sequence);
        if pending.is_empty() {
            self.last_barrier = None;
            return cycle;
        }

        let mut schedulable = Vec::new();
        for id in pending {
            let reservation = &self.reservations[&id];
            let age_ms = now.elapsed_since(reservation.enqueued_at);
            let due = age_ms >= max_wait_ms;
            if let Some(reason) = self.static_reason(&reservation.request) {
                self.record(
                    &mut cycle,
                    id,
                    PendingStatus::Unschedulable {
                        reason,
                        age_ms,
                        due,
                    },
                );
            } else {
                schedulable.push(id);
            }
        }

        let barrier = schedulable
            .iter()
            .copied()
            .filter(|id| {
                now.elapsed_since(self.reservations[id].enqueued_at) >= max_wait_ms
            })
            .map(|id| self.reservations[&id].sequence)
            .min();
        self.last_barrier = barrier;
        cycle.barrier = barrier;

        let mut eligible = Vec::new();
        for id in schedulable {
            let reservation = &self.reservations[&id];
            let age_ms = now.elapsed_since(reservation.enqueued_at);
            let due = age_ms >= max_wait_ms;
            if let Some(limit) = barrier {
                if reservation.sequence > limit {
                    self.record(
                        &mut cycle,
                        id,
                        PendingStatus::Waiting {
                            reason: PendingReason::BehindBarrier {
                                barrier_sequence: limit,
                            },
                            age_ms,
                            due,
                        },
                    );
                    continue;
                }
            }
            eligible.push(id);
        }

        eligible.sort_by(|left, right| {
            let a = &self.reservations[left];
            let b = &self.reservations[right];
            let a_due = now.elapsed_since(a.enqueued_at) >= max_wait_ms;
            let b_due = now.elapsed_since(b.enqueued_at) >= max_wait_ms;
            match (a_due, b_due) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                (true, true) => a.sequence.cmp(&b.sequence),
                (false, false) => {
                    let area_a = u64::from(a.request.shape.vcpu().get())
                        * u64::from(a.request.shape.memory_gib());
                    let area_b = u64::from(b.request.shape.vcpu().get())
                        * u64::from(b.request.shape.memory_gib());
                    area_a.cmp(&area_b).then(a.sequence.cmp(&b.sequence))
                }
            }
        });

        for id in eligible {
            let (request, enqueued_at) = {
                let reservation = &self.reservations[&id];
                (reservation.request.clone(), reservation.enqueued_at)
            };
            let age_ms = now.elapsed_since(enqueued_at);
            let due = age_ms >= max_wait_ms;

            if !due {
                if let Some(zone) = &request.preferred_cache_zone {
                    let zone_hosts: Vec<HostKey> = self
                        .hosts
                        .values()
                        .filter(|host| {
                            host.admission.is_eligible()
                                && host.arch == request.arch
                                && host.cache_zone.as_ref() == Some(zone)
                        })
                        .map(|host| host.host_key.clone())
                        .collect();
                    if !zone_hosts.is_empty() {
                        let overhead = self.hosts[&zone_hosts[0]].vmm_overhead;
                        let cost = request.cost(overhead);
                        let all_full = zone_hosts.iter().all(|host_key| {
                            let host = &self.hosts[host_key];
                            !Self::fits(host, cost, &usage[host_key])
                        });
                        if all_full {
                            self.record(
                                &mut cycle,
                                id,
                                PendingStatus::Waiting {
                                    reason: PendingReason::AwaitingCacheLocality {
                                        zone: zone.clone(),
                                    },
                                    age_ms,
                                    due,
                                },
                            );
                            continue;
                        }
                    }
                }
            }

            if let Some(host_key) = self.candidate_hosts(&request, &usage).into_iter().next() {
                let offer = self.grant(id, &host_key, &mut usage);
                cycle.offers.push(offer.clone());
                self.record(&mut cycle, id, PendingStatus::Granted { offer });
                continue;
            }

            if due {
                if let Some((host_key, reclaimed)) = self.reclaim_plan(&request, &usage) {
                    for reservation_id in &reclaimed {
                        self.release_internal(*reservation_id, &mut usage);
                    }
                    cycle.reclaimed.extend(reclaimed);
                    let offer = self.grant(id, &host_key, &mut usage);
                    cycle.offers.push(offer.clone());
                    self.record(&mut cycle, id, PendingStatus::Granted { offer });
                    continue;
                }
            }

            let reason = self.best_failure_reason(&request);
            self.record(
                &mut cycle,
                id,
                PendingStatus::Unschedulable {
                    reason,
                    age_ms,
                    due,
                },
            );
        }

        cycle
    }

    pub(crate) fn restore_host(&mut self, capacity: HostCapacity) {
        self.hosts.insert(capacity.host_key.clone(), capacity);
    }

    pub(crate) fn restore_reservation(&mut self, reservation: Reservation) {
        self.next_id = self.next_id.max(reservation.id.get() + 1);
        self.next_sequence = self.next_sequence.max(reservation.sequence + 1);
        self.reservations.insert(reservation.id, reservation);
    }

    fn record(&mut self, cycle: &mut Cycle, id: ReservationId, status: PendingStatus) {
        self.statuses.insert(id, status.clone());
        cycle.statuses.push((id, status));
    }

    fn transition(
        &mut self,
        id: ReservationId,
        incarnation: HostIncarnation,
        to: ReservationState,
        allowed: &[ReservationState],
    ) -> Result<(), LedgerError> {
        let reservation = self
            .reservations
            .get_mut(&id)
            .ok_or(LedgerError::UnknownReservation(id))?;
        if reservation.incarnation != incarnation {
            return Err(LedgerError::ReservationIncarnationMismatch {
                reservation_id: id,
                expected: reservation.incarnation.get(),
                found: incarnation.get(),
            });
        }
        if !allowed.contains(&reservation.state) {
            return Err(LedgerError::InvalidStateTransition {
                reservation_id: id,
                from: reservation.state,
                to,
            });
        }
        reservation.state = to;
        Ok(())
    }

    fn compute_usage(&self) -> BTreeMap<HostKey, Usage> {
        let mut usage: BTreeMap<HostKey, Usage> = self
            .hosts
            .keys()
            .map(|host_key| (host_key.clone(), Usage::default()))
            .collect();
        for reservation in self
            .reservations
            .values()
            .filter(|reservation| reservation.state.occupies_capacity())
        {
            let Some(host_key) = &reservation.host else {
                continue;
            };
            let overhead = self
                .hosts
                .get(host_key)
                .map(|host| host.vmm_overhead)
                .unwrap_or(VmmOverhead::zero());
            let cost = reservation.request.cost(overhead);
            let used = usage.entry(host_key.clone()).or_default();
            used.vcpu_milli = used.vcpu_milli.saturating_add(cost.vcpu_milli);
            used.memory_mib = used.memory_mib.saturating_add(cost.memory_mib);
            used.disk_bytes = used.disk_bytes.saturating_add(cost.disk_bytes);
        }
        usage
    }

    fn fits(host: &HostCapacity, cost: DemandCost, used: &Usage) -> bool {
        used.vcpu_milli.saturating_add(cost.vcpu_milli) <= host.reservable_vcpu_milli()
            && used.memory_mib.saturating_add(cost.memory_mib) <= host.reservable_memory_mib()
            && used.disk_bytes.saturating_add(cost.disk_bytes) <= host.reservable_disk_bytes()
    }

    fn static_reason(&self, request: &ReservationRequest) -> Option<UnschedulableReason> {
        if self.hosts.is_empty() {
            return Some(UnschedulableReason::NoHostsRegistered);
        }
        let matching: Vec<&HostCapacity> = self
            .hosts
            .values()
            .filter(|host| host.admission.is_eligible() && host.arch == request.arch)
            .collect();
        if matching.is_empty() {
            return Some(UnschedulableReason::ArchitectureMismatch {
                required: request.arch.as_label_token(),
            });
        }
        let max_vcpu = matching
            .iter()
            .map(|host| host.reservable_vcpu_milli() / 1000)
            .max()
            .unwrap_or(0);
        let max_memory_gib = matching
            .iter()
            .map(|host| host.reservable_memory_mib() / 1024)
            .max()
            .unwrap_or(0);
        let shape_fits_somewhere = matching.iter().any(|host| {
            host.reservable_vcpu_milli() >= u64::from(request.shape.vcpu().get()) * 1000
                && host.reservable_memory_mib() >= u64::from(request.shape.memory_gib()) * 1024
                && host.reservable_disk_bytes() >= request.disk_bytes
        });
        if !shape_fits_somewhere {
            return Some(UnschedulableReason::ShapeExceedsHostCapacity {
                shape: request.shape,
                max_vcpu: u32::try_from(max_vcpu).unwrap_or(u32::MAX),
                max_memory_gib: u32::try_from(max_memory_gib).unwrap_or(u32::MAX),
            });
        }
        None
    }

    fn candidate_hosts(&self, request: &ReservationRequest, usage: &BTreeMap<HostKey, Usage>) -> Vec<HostKey> {
        let mut candidates = Vec::new();
        for host in self.hosts.values() {
            if !host.admission.is_eligible() || host.arch != request.arch {
                continue;
            }
            let cost = request.cost(host.vmm_overhead);
            if !Self::fits(host, cost, &usage[&host.host_key]) {
                continue;
            }
            let remaining_cpu = host
                .reservable_vcpu_milli()
                .saturating_sub(usage[&host.host_key].vcpu_milli)
                .saturating_sub(cost.vcpu_milli);
            let remaining_memory = host
                .reservable_memory_mib()
                .saturating_sub(usage[&host.host_key].memory_mib)
                .saturating_sub(cost.memory_mib);
            let cache_mismatch = request.preferred_cache_zone.as_ref() != host.cache_zone.as_ref();
            candidates.push((cache_mismatch, remaining_cpu.min(remaining_memory), host.host_key.clone()));
        }
        candidates.sort();
        candidates.into_iter().map(|(_, _, host_key)| host_key).collect()
    }

    fn reclaim_plan(
        &self,
        request: &ReservationRequest,
        usage: &BTreeMap<HostKey, Usage>,
    ) -> Option<(HostKey, Vec<ReservationId>)> {
        let mut candidates = Vec::new();
        for host in self.hosts.values() {
            if !host.admission.is_eligible() || host.arch != request.arch {
                continue;
            }
            let cost = request.cost(host.vmm_overhead);
            let mut reclaim = Usage::default();
            let mut ids = Vec::new();
            for reservation in self.reservations.values().filter(|reservation| {
                reservation.state == ReservationState::IdleWarm
                    && reservation.host.as_ref() == Some(&host.host_key)
            }) {
                let reservation_cost = reservation.request.cost(host.vmm_overhead);
                reclaim.vcpu_milli = reclaim.vcpu_milli.saturating_add(reservation_cost.vcpu_milli);
                reclaim.memory_mib = reclaim.memory_mib.saturating_add(reservation_cost.memory_mib);
                reclaim.disk_bytes = reclaim.disk_bytes.saturating_add(reservation_cost.disk_bytes);
                ids.push(reservation.id);
            }
            if ids.is_empty() {
                continue;
            }
            let used = &usage[&host.host_key];
            let after_reclaim = Usage {
                vcpu_milli: used.vcpu_milli.saturating_sub(reclaim.vcpu_milli),
                memory_mib: used.memory_mib.saturating_sub(reclaim.memory_mib),
                disk_bytes: used.disk_bytes.saturating_sub(reclaim.disk_bytes),
            };
            if Self::fits(host, cost, &after_reclaim) {
                let cache_mismatch =
                    request.preferred_cache_zone.as_ref() != host.cache_zone.as_ref();
                candidates.push((cache_mismatch, host.host_key.clone(), ids));
            }
        }
        candidates.sort();
        candidates
            .into_iter()
            .next()
            .map(|(_, host_key, ids)| (host_key, ids))
    }

    fn best_failure_reason(&self, request: &ReservationRequest) -> UnschedulableReason {
        let mut matching: Vec<&HostCapacity> = self
            .hosts
            .values()
            .filter(|host| host.admission.is_eligible() && host.arch == request.arch)
            .collect();
        matching.sort_by(|left, right| {
            let left_match = request.preferred_cache_zone.as_ref() == left.cache_zone.as_ref();
            let right_match = request.preferred_cache_zone.as_ref() == right.cache_zone.as_ref();
            right_match.cmp(&left_match).then(left.host_key.cmp(&right.host_key))
        });
        let Some(host) = matching.first() else {
            return UnschedulableReason::ArchitectureMismatch {
                required: request.arch.as_label_token(),
            };
        };
        let has_idle_warm = self.reservations.values().any(|reservation| {
            reservation.state == ReservationState::IdleWarm
                && reservation.host.as_ref() == Some(&host.host_key)
        });
        if has_idle_warm {
            UnschedulableReason::NoReclaimableCapacity {
                host_key: host.host_key.clone(),
                shape: request.shape,
            }
        } else {
            UnschedulableReason::InsufficientCapacity {
                host_key: host.host_key.clone(),
                shape: request.shape,
            }
        }
    }

    fn grant(
        &mut self,
        id: ReservationId,
        host_key: &HostKey,
        usage: &mut BTreeMap<HostKey, Usage>,
    ) -> CapacityOffer {
        let (incarnation, cost) = {
            let host = &self.hosts[host_key];
            let reservation = &self.reservations[&id];
            (host.incarnation, reservation.request.cost(host.vmm_overhead))
        };
        let reservation = self.reservations.get_mut(&id).expect("reservation exists");
        reservation.state = ReservationState::Reserved;
        reservation.host = Some(host_key.clone());
        reservation.incarnation = incarnation;
        let used = usage.entry(host_key.clone()).or_default();
        used.vcpu_milli = used.vcpu_milli.saturating_add(cost.vcpu_milli);
        used.memory_mib = used.memory_mib.saturating_add(cost.memory_mib);
        used.disk_bytes = used.disk_bytes.saturating_add(cost.disk_bytes);
        CapacityOffer {
            reservation_id: id,
            host_key: host_key.clone(),
            incarnation,
            revision_id: reservation.request.revision_id,
            shape: reservation.request.shape,
            disk_bytes: reservation.request.disk_bytes,
        }
    }

    fn release_internal(&mut self, id: ReservationId, usage: &mut BTreeMap<HostKey, Usage>) {
        let (host_key, cost) = {
            let reservation = &self.reservations[&id];
            let host_key = reservation.host.clone().expect("active reservation has a host");
            let overhead = self
                .hosts
                .get(&host_key)
                .map(|host| host.vmm_overhead)
                .unwrap_or(VmmOverhead::zero());
            (host_key, reservation.request.cost(overhead))
        };
        let reservation = self.reservations.get_mut(&id).expect("reservation exists");
        reservation.state = ReservationState::Released;
        if let Some(used) = usage.get_mut(&host_key) {
            used.vcpu_milli = used.vcpu_milli.saturating_sub(cost.vcpu_milli);
            used.memory_mib = used.memory_mib.saturating_sub(cost.memory_mib);
            used.disk_bytes = used.disk_bytes.saturating_sub(cost.disk_bytes);
        }
        self.statuses.remove(&id);
    }

    fn offer_for(&self, reservation: &Reservation, host_key: &HostKey) -> CapacityOffer {
        let host = &self.hosts[host_key];
        CapacityOffer {
            reservation_id: reservation.id,
            host_key: host_key.clone(),
            incarnation: host.incarnation,
            revision_id: reservation.request.revision_id,
            shape: reservation.request.shape,
            disk_bytes: reservation.request.disk_bytes,
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::domain::{
        Arch, Distro, ImageTarget, Profile, ProfileName, ProfileRevision, ReleaseToken, Shape,
    };

    pub(crate) struct Rng(u64);

    impl Rng {
        pub(crate) const fn seed(seed: u64) -> Self {
            Self(seed)
        }

        pub(crate) fn next(&mut self) -> u64 {
            let mut value = self.0;
            value ^= value << 13;
            value ^= value >> 7;
            value ^= value << 17;
            self.0 = value;
            value
        }

        pub(crate) fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
    }

    pub(crate) fn revision(name: &str, vcpu: u16, memory_gib: u32) -> ProfileRevision {
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

    pub(crate) fn host(
        key: &str,
        incarnation: u64,
        vcpu: u32,
        memory_gib: u32,
    ) -> HostCapacity {
        HostCapacity {
            host_key: HostKey::new(key),
            incarnation: HostIncarnation::new(incarnation),
            hostname: key.to_owned(),
            arch: Arch::Arm64,
            total_vcpu: vcpu,
            total_memory_gib: memory_gib,
            total_disk_bytes: 1_000_000 * 1024 * 1024,
            headroom_vcpu: 0,
            headroom_memory_gib: 0,
            headroom_disk_bytes: 0,
            vmm_overhead: VmmOverhead::zero(),
            admission: crate::ledger::types::HostAdmission::Eligible,
            cache_zone: None,
            last_contact: Tick::new(0),
        }
    }
}
