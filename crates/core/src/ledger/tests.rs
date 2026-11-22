use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use rusqlite::{Connection, params};

use super::engine::test_support::{Rng, host, revision};
use super::*;

fn grant(state: &mut LedgerState, request: ReservationRequest, now: Tick) -> CapacityOffer {
    match state.reserve(request, now) {
        ReservationOutcome::Granted(offer) => offer,
        ReservationOutcome::Pending { status, .. } => panic!("expected a grant, got {status:?}"),
    }
}

#[test]
fn ledger_never_overcommits_ram_and_honors_headroom_and_overhead() {
    let mut state = LedgerState::new(LedgerConfig::default());
    let mut capacity = host("h1", 1, 8, 16);
    capacity.headroom_vcpu = 2;
    capacity.headroom_memory_gib = 4;
    capacity.vmm_overhead = VmmOverhead::new(128, 50);
    state.register_host(capacity).expect("register host");

    let profile = revision("small-2c4g", 2, 4);
    assert_eq!(state.offers_for(&profile).slots, 2);

    let request = ReservationRequest::new(&profile, 0, None);
    grant(&mut state, request.clone(), Tick::new(0));
    grant(&mut state, request.clone(), Tick::new(0));
    let third = state.reserve(request, Tick::new(0));
    assert!(matches!(
        third,
        ReservationOutcome::Pending {
            status: PendingStatus::Unschedulable { .. },
            ..
        }
    ));

    let usage = state.usage(&HostKey::new("h1"));
    assert_eq!(usage.vcpu_milli, 2 * (2 * 1000 + 50));
    assert_eq!(usage.memory_mib, 2 * (4 * 1024 + 128));
    assert!(usage.vcpu_milli <= 6 * 1000);
    assert!(usage.memory_mib <= 12 * 1024);
    assert_eq!(state.offers_for(&profile).slots, 0);
}

#[test]
fn ledger_fences_stale_incarnations_for_reserve_and_release() {
    let mut state = LedgerState::new(LedgerConfig::default());
    state.register_host(host("h1", 1, 8, 16)).expect("register");
    let key = HostKey::new("h1");
    let offer = grant(
        &mut state,
        ReservationRequest::new(&revision("one", 1, 2), 0, None),
        Tick::new(0),
    );

    assert!(matches!(
        state.register_host(host("h1", 0, 64, 128)),
        Err(LedgerError::StaleHostIncarnation { .. })
    ));
    assert!(matches!(
        state.release(
            offer.reservation_id,
            &key,
            HostIncarnation::new(0),
            Tick::new(1)
        ),
        Err(LedgerError::StaleHostIncarnation { .. })
    ));
    assert_eq!(state.usage(&key).vcpu_milli, 1000);

    state
        .release(
            offer.reservation_id,
            &key,
            HostIncarnation::new(1),
            Tick::new(2),
        )
        .expect("current incarnation releases its own reservation");
    assert_eq!(state.usage(&key).vcpu_milli, 0);
}

#[test]
fn ledger_newer_incarnation_cannot_free_a_previous_incarnations_reservation() {
    let mut state = LedgerState::new(LedgerConfig::default());
    state.register_host(host("h1", 1, 8, 16)).expect("register");
    let key = HostKey::new("h1");
    let offer = grant(
        &mut state,
        ReservationRequest::new(&revision("one", 1, 2), 0, None),
        Tick::new(0),
    );
    state.register_host(host("h1", 2, 8, 16)).expect("reincarnate");

    assert!(matches!(
        state.release(
            offer.reservation_id,
            &key,
            HostIncarnation::new(2),
            Tick::new(1)
        ),
        Err(LedgerError::StaleHostIncarnation { .. })
            | Err(LedgerError::ReservationIncarnationMismatch { .. })
    ));
    assert_eq!(state.usage(&key).vcpu_milli, 1000);
}

#[test]
fn ledger_heartbeat_expiry_never_frees_capacity() {
    let mut state = LedgerState::new(LedgerConfig::default());
    state.register_host(host("h1", 1, 8, 16)).expect("register");
    let key = HostKey::new("h1");
    let request = ReservationRequest::new(&revision("small", 1, 2), 0, None);
    let first = grant(&mut state, request.clone(), Tick::new(0));
    state
        .note_heartbeat(&key, HostIncarnation::new(1), Tick::new(0))
        .expect("heartbeat");

    assert_eq!(state.expired_hosts(Tick::new(60_000), 30_000), vec![key.clone()]);
    assert_eq!(state.usage(&key).vcpu_milli, 1000);
    assert_eq!(
        state.reservation(first.reservation_id).unwrap().state,
        ReservationState::Reserved
    );

    let cycle = state.plan(Tick::new(120_000));
    assert!(cycle.offers.is_empty());
    assert!(cycle.reclaimed.is_empty());
    grant(&mut state, request, Tick::new(120_000));
    assert_eq!(state.usage(&key).vcpu_milli, 2000);
}

#[test]
fn ledger_cache_locality_preference_is_capped_at_max_wait() {
    let mut state = LedgerState::new(LedgerConfig {
        max_wait: Duration::from_millis(1000),
    });
    let mut zone_a = host("a", 1, 4, 8);
    zone_a.cache_zone = Some(CacheZone::new("zone-a"));
    let mut zone_b = host("b", 1, 4, 8);
    zone_b.cache_zone = Some(CacheZone::new("zone-b"));
    state.register_host(zone_a).expect("register a");
    state.register_host(zone_b).expect("register b");

    let fill = ReservationRequest::new(&revision("fill", 4, 8), 0, Some(CacheZone::new("zone-a")));
    let fill_offer = grant(&mut state, fill, Tick::new(0));
    assert_eq!(fill_offer.host_key.as_str(), "a");

    let request = ReservationRequest::new(&revision("main", 1, 2), 0, Some(CacheZone::new("zone-a")));
    let id = state.submit(request, Tick::new(0));
    assert!(state.plan(Tick::new(0)).offers.is_empty());
    assert!(matches!(
        state.status(id, Tick::new(0)),
        Some(PendingStatus::Waiting {
            reason: PendingReason::AwaitingCacheLocality { .. },
            ..
        })
    ));
    assert!(state.plan(Tick::new(999)).offers.is_empty());

    let cycle = state.plan(Tick::new(1000));
    assert_eq!(cycle.offers.len(), 1);
    assert_eq!(cycle.offers[0].host_key.as_str(), "b");
}

#[test]
fn ledger_feasible_large_profile_waits_at_most_max_wait_behind_small_arrivals() {
    let max_wait_ms = 10_000;
    let mut state = LedgerState::new(LedgerConfig {
        max_wait: Duration::from_millis(max_wait_ms),
    });
    state.register_host(host("big", 1, 64, 128)).expect("register");
    let key = HostKey::new("big");

    let small_request = ReservationRequest::new(&revision("small-1c2g", 1, 2), 0, None);
    for _ in 0..64 {
        let offer = grant(&mut state, small_request.clone(), Tick::new(0));
        state
            .mark_idle_warm(offer.reservation_id, HostIncarnation::new(1))
            .expect("warm");
    }

    let large = state.submit(
        ReservationRequest::new(&revision("large-64c128g", 64, 128), 0, None),
        Tick::new(0),
    );
    let mut granted_at = None;
    let mut reclaimed = 0usize;
    for arrival in 1..=1000u64 {
        let now = Tick::new(arrival * 10);
        state.submit(small_request.clone(), now);
        let cycle = state.plan(now);
        reclaimed += cycle.reclaimed.len();
        if granted_at.is_none() && cycle.offers.iter().any(|offer| offer.reservation_id == large) {
            granted_at = Some(now.get());
        }
    }

    let granted_at = granted_at.expect("large profile is eventually granted");
    assert!(granted_at >= max_wait_ms, "large profile must not be granted early");
    assert!(
        granted_at <= max_wait_ms,
        "large profile waited {granted_at}ms, beyond max_wait {max_wait_ms}ms"
    );
    assert_eq!(reclaimed, 64);
    assert_eq!(state.usage(&key).vcpu_milli, 64 * 1000);
}

#[test]
fn ledger_reclaim_only_releases_idle_warm_capacity() {
    let mut state = LedgerState::new(LedgerConfig {
        max_wait: Duration::from_millis(1000),
    });
    let mut host_a = host("a", 1, 10, 20);
    host_a.cache_zone = Some(CacheZone::new("za"));
    let mut host_b = host("b", 1, 8, 16);
    host_b.cache_zone = Some(CacheZone::new("zb"));
    state.register_host(host_a).expect("register a");
    state.register_host(host_b).expect("register b");

    let small_request = |zone: &str| {
        ReservationRequest::new(&revision("small", 1, 2), 0, Some(CacheZone::new(zone)))
    };

    let mut busy = Vec::new();
    let mut warm = Vec::new();
    for _ in 0..2 {
        let offer = grant(&mut state, small_request("za"), Tick::new(0));
        state.mark_busy(offer.reservation_id, HostIncarnation::new(1)).unwrap();
        busy.push(offer.reservation_id);
    }
    for _ in 0..4 {
        let offer = grant(&mut state, small_request("za"), Tick::new(0));
        state.mark_idle_warm(offer.reservation_id, HostIncarnation::new(1)).unwrap();
        warm.push(offer.reservation_id);
    }
    for _ in 0..8 {
        let offer = grant(&mut state, small_request("zb"), Tick::new(0));
        state.mark_busy(offer.reservation_id, HostIncarnation::new(1)).unwrap();
        busy.push(offer.reservation_id);
    }

    let large_request = ReservationRequest::new(&revision("large", 8, 16), 0, None);
    let grantable = state.submit(large_request.clone(), Tick::new(0));
    let blocked = state.submit(large_request, Tick::new(0));
    let cycle = state.plan(Tick::new(1000));

    assert_eq!(cycle.offers.len(), 1);
    assert_eq!(cycle.offers[0].host_key.as_str(), "a");
    assert_eq!(cycle.offers[0].reservation_id, grantable);
    assert_eq!(cycle.reclaimed.len(), 4);
    assert!(matches!(
        state.status(blocked, Tick::new(1000)),
        Some(PendingStatus::Waiting {
            reason: PendingReason::BehindBarrier { .. },
            ..
        }) | Some(PendingStatus::Unschedulable { .. })
    ));
    for id in warm {
        assert_eq!(state.reservation(id).unwrap().state, ReservationState::Released);
    }
    for id in busy {
        assert_eq!(state.reservation(id).unwrap().state, ReservationState::Busy);
    }
}

#[test]
fn ledger_impossible_shape_does_not_block_feasible_ones() {
    let mut state = LedgerState::new(LedgerConfig {
        max_wait: Duration::from_millis(1000),
    });
    state.register_host(host("h1", 1, 8, 16)).expect("register");

    let impossible = state.reserve(
        ReservationRequest::new(&revision("huge", 128, 256), 0, None),
        Tick::new(0),
    );
    let feasible = state.reserve(
        ReservationRequest::new(&revision("small", 1, 2), 0, None),
        Tick::new(0),
    );
    match impossible {
        ReservationOutcome::Pending {
            status:
                PendingStatus::Unschedulable {
                    reason: UnschedulableReason::ShapeExceedsHostCapacity { .. },
                    ..
                },
            ..
        } => {}
        other => panic!("expected an impossible-shape reason, got {other:?}"),
    }
    assert!(matches!(feasible, ReservationOutcome::Granted(_)));

    let overdue = state.plan(Tick::new(1000));
    assert_eq!(overdue.barrier, None);
    let another = state.reserve(
        ReservationRequest::new(&revision("medium", 2, 4), 0, None),
        Tick::new(1000),
    );
    assert!(matches!(another, ReservationOutcome::Granted(_)));
}

#[test]
fn ledger_property_fixed_seed_never_double_promises() {
    let mut rng = Rng::seed(0x9E37_79B9_7F4A_7C15);
    let mut state = LedgerState::new(LedgerConfig {
        max_wait: Duration::from_millis(500),
    });
    let mut constrained = host("a", 1, 16, 32);
    constrained.headroom_vcpu = 2;
    constrained.headroom_memory_gib = 4;
    constrained.vmm_overhead = VmmOverhead::new(64, 25);
    state.register_host(constrained).expect("register a");
    state.register_host(host("b", 1, 8, 16)).expect("register b");
    state.register_host(host("c", 1, 32, 64)).expect("register c");

    let shapes = [(1, 2), (2, 4), (4, 8), (8, 16), (16, 32), (32, 64), (64, 128)];
    let profiles: Vec<_> = shapes
        .iter()
        .enumerate()
        .map(|(index, (vcpu, memory))| revision(&format!("p{index}"), *vcpu, *memory))
        .collect();
    let mut active: Vec<(ReservationId, HostKey, HostIncarnation)> = Vec::new();

    for step in 0..600u64 {
        let now = Tick::new(step * 10);
        match rng.below(10) {
            0..=4 => {
                let profile = &profiles[rng.below(profiles.len() as u64) as usize];
                let request = ReservationRequest::new(profile, rng.below(4) * 1024, None);
                if let ReservationOutcome::Granted(offer) = state.reserve(request, now) {
                    active.push((offer.reservation_id, offer.host_key, offer.incarnation));
                }
            }
            5..=6 => {
                if !active.is_empty() {
                    let index = rng.below(active.len() as u64) as usize;
                    let (id, key, incarnation) = active[index].clone();
                    if state.release(id, &key, incarnation, now).is_ok() {
                        active.swap_remove(index);
                    }
                }
            }
            7 => {
                if let Some((id, _, incarnation)) = active.first().cloned() {
                    let _ = state.mark_busy(id, incarnation);
                }
            }
            8 => {
                if let Some((id, _, incarnation)) = active.last().cloned() {
                    let _ = state.mark_idle_warm(id, incarnation);
                }
            }
            _ => {
                state.plan(now);
            }
        }

        let mut shadow: BTreeMap<HostKey, Usage> = BTreeMap::new();
        for reservation in state.reservations().filter(|r| r.state.occupies_capacity()) {
            let key = reservation.host.clone().expect("active reservation has a host");
            let overhead = state.host(&key).expect("host is registered").vmm_overhead;
            let cost = reservation.request.cost(overhead);
            let used = shadow.entry(key).or_default();
            used.vcpu_milli += cost.vcpu_milli;
            used.memory_mib += cost.memory_mib;
            used.disk_bytes += cost.disk_bytes;
        }
        for registered in state.hosts() {
            let used = shadow.get(&registered.host_key).cloned().unwrap_or_default();
            assert!(
                used.vcpu_milli <= registered.reservable_vcpu_milli(),
                "vCPU overcommit on {} at step {step}",
                registered.host_key
            );
            assert!(
                used.memory_mib <= registered.reservable_memory_mib(),
                "RAM overcommit on {} at step {step}",
                registered.host_key
            );
            assert!(
                used.disk_bytes <= registered.reservable_disk_bytes(),
                "disk overcommit on {} at step {step}",
                registered.host_key
            );
            assert_eq!(used, state.usage(&registered.host_key));
        }
        let ids: HashSet<ReservationId> = state.reservations().map(|reservation| reservation.id).collect();
        assert_eq!(ids.len(), state.reservations().count());
    }
}

#[test]
fn ledger_sqlite_persists_grants_and_fences_release_by_incarnation() {
    let mut ledger = Ledger::from_connection(Connection::open_in_memory().expect("in-memory db"))
        .expect("ledger over in-memory sqlite");
    ledger.register_host(host("h1", 1, 8, 16)).expect("register");
    let key = HostKey::new("h1");
    let offer = match ledger
        .reserve(
            ReservationRequest::new(&revision("small", 2, 4), 0, None),
            Tick::new(5),
        )
        .expect("reserve")
    {
        ReservationOutcome::Granted(offer) => offer,
        ReservationOutcome::Pending { status, .. } => panic!("expected grant, got {status:?}"),
    };

    {
        let stored: (String, i64) = ledger
            .connection()
            .unwrap()
            .query_row(
                "SELECT state, incarnation FROM reservation WHERE id = ?1",
                params![i64::try_from(offer.reservation_id.get()).unwrap()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("stored reservation");
        assert_eq!(stored, ("reserved".to_owned(), 1));
    }

    assert!(matches!(
        ledger.release(
            offer.reservation_id,
            &key,
            HostIncarnation::new(0),
            Tick::new(6)
        ),
        Err(LedgerError::StaleHostIncarnation { .. })
    ));
    {
        let stored: String = ledger
            .connection()
            .unwrap()
            .query_row(
                "SELECT state FROM reservation WHERE id = ?1",
                params![i64::try_from(offer.reservation_id.get()).unwrap()],
                |row| row.get(0),
            )
            .expect("stored reservation");
        assert_eq!(stored, "reserved");
    }

    ledger
        .release(
            offer.reservation_id,
            &key,
            HostIncarnation::new(1),
            Tick::new(7),
        )
        .expect("current incarnation releases");
    let stored: (String, Option<String>) = ledger
        .connection()
        .unwrap()
        .query_row(
            "SELECT state, released_at FROM reservation WHERE id = ?1",
            params![i64::try_from(offer.reservation_id.get()).unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("stored reservation");
    assert_eq!(stored.0, "released");
    assert_eq!(stored.1.as_deref(), Some("7"));
}

#[test]
fn ledger_sqlite_restores_capacity_metadata_and_active_reservations() {
    let mut ledger = Ledger::from_connection(Connection::open_in_memory().expect("in-memory db"))
        .expect("ledger over in-memory sqlite");
    let key = HostKey::new("h1");
    let mut capacity = host("h1", 1, 16, 32);
    capacity.headroom_vcpu = 2;
    capacity.headroom_memory_gib = 4;
    capacity.vmm_overhead = VmmOverhead::new(128, 50);
    capacity.cache_zone = Some(CacheZone::new("zone-a"));
    capacity.last_contact = Tick::new(7);
    ledger.register_host(capacity).expect("register");
    let offer = match ledger
        .reserve(
            ReservationRequest::new(&revision("small", 2, 4), 0, None),
            Tick::new(7),
        )
        .expect("reserve")
    {
        ReservationOutcome::Granted(offer) => offer,
        ReservationOutcome::Pending { status, .. } => panic!("expected grant, got {status:?}"),
    };

    let connection = ledger.into_connection().expect("connection");
    let mut reopened = Ledger::from_connection(connection).expect("reopen ledger");
    let restored = reopened.host(&key).expect("restored host");
    assert_eq!(restored.headroom_vcpu, 2);
    assert_eq!(restored.headroom_memory_gib, 4);
    assert_eq!(restored.vmm_overhead, VmmOverhead::new(128, 50));
    assert_eq!(restored.cache_zone, Some(CacheZone::new("zone-a")));
    assert_eq!(restored.last_contact, Tick::new(7));
    assert_eq!(reopened.usage(&key).vcpu_milli, 2 * 1000 + 50);
    assert_eq!(reopened.usage(&key).memory_mib, 4 * 1024 + 128);
    assert_eq!(
        reopened.reservation(offer.reservation_id).unwrap().state,
        ReservationState::Reserved
    );
    assert!(matches!(
        reopened.register_host(host("h1", 0, 64, 128)),
        Err(LedgerError::StaleHostIncarnation { .. })
    ));
}

#[test]
fn host_allowance_matches_the_reserved_cost_for_the_shape() {
    use crate::domain::Shape;
    use crate::ledger::{HostAllowance, VmmOverhead};
    let shape = Shape::new(4, 8).expect("shape");
    let allowance = HostAllowance::for_shape(shape, VmmOverhead::new(512, 250));
    assert_eq!(allowance.cpu_max(), "425000 100000");
    assert_eq!(allowance.memory_bytes, (8 * 1024 + 512) * 1024 * 1024);
    let cost = crate::ledger::types::demand_cost(shape, VmmOverhead::new(512, 250), 0);
    assert_eq!(allowance.cpu_quota_us, cost.vcpu_milli * 100);
    assert_eq!(allowance.memory_bytes, cost.memory_mib * 1024 * 1024);
}

#[test]
fn a_reopened_ledger_never_reuses_a_released_reservation_id() {
    let mut ledger = Ledger::from_connection(Connection::open_in_memory().expect("in-memory db"))
        .expect("ledger over in-memory sqlite");
    let key = HostKey::new("h1");
    ledger.register_host(host("h1", 1, 16, 32)).expect("register");
    let grant = |ledger: &mut Ledger| match ledger
        .reserve(ReservationRequest::new(&revision("small", 2, 4), 0, None), Tick::new(7))
        .expect("reserve")
    {
        ReservationOutcome::Granted(offer) => offer,
        ReservationOutcome::Pending { status, .. } => panic!("expected grant, got {status:?}"),
    };
    let first = grant(&mut ledger);
    ledger
        .release(first.reservation_id, &key, HostIncarnation::new(1), Tick::new(8))
        .expect("release");
    let connection = ledger.into_connection().expect("connection");
    let mut reopened = Ledger::from_connection(connection).expect("reopen ledger");
    let second = grant(&mut reopened);
    assert!(second.reservation_id.get() > first.reservation_id.get());
}
