use rusqlite::{Connection, params};

use crate::domain::{Arch, Shape};
use crate::ledger::engine::LedgerState;
use crate::ledger::error::LedgerError;
use crate::ledger::types::{
    CacheZone, HostAdmission, HostCapacity, HostIncarnation, HostKey, Reservation,
    ReservationState, Tick, VmmOverhead,
};
use crate::store::{StoreError, migrate};

fn parse_tick(text: Option<String>) -> Tick {
    text.and_then(|value| value.parse::<u64>().ok())
        .map(Tick::new)
        .unwrap_or(Tick::new(0))
}

fn tick_text(tick: Tick) -> String {
    tick.get().to_string()
}

fn metadata(capacity: &HostCapacity) -> String {
    let cache_zone = capacity.cache_zone.as_ref().map(CacheZone::as_str);
    serde_json::json!({
        "headroom_vcpu": capacity.headroom_vcpu,
        "headroom_memory_gib": capacity.headroom_memory_gib,
        "headroom_disk_bytes": capacity.headroom_disk_bytes,
        "vmm_memory_mib": capacity.vmm_overhead.measured_memory_mib,
        "vmm_vcpu_milli": capacity.vmm_overhead.measured_vcpu_milli,
        "cache_zone": cache_zone,
        "last_contact_ms": capacity.last_contact.get(),
    })
    .to_string()
}

fn apply_metadata(capacity: &mut HostCapacity, text: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    capacity.headroom_vcpu = value
        .get("headroom_vcpu")
        .and_then(serde_json::Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .unwrap_or(capacity.headroom_vcpu);
    capacity.headroom_memory_gib = value
        .get("headroom_memory_gib")
        .and_then(serde_json::Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .unwrap_or(capacity.headroom_memory_gib);
    capacity.headroom_disk_bytes = value
        .get("headroom_disk_bytes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(capacity.headroom_disk_bytes);
    capacity.vmm_overhead = VmmOverhead::new(
        value
            .get("vmm_memory_mib")
            .and_then(serde_json::Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
            .unwrap_or(capacity.vmm_overhead.measured_memory_mib),
        value
            .get("vmm_vcpu_milli")
            .and_then(serde_json::Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
            .unwrap_or(capacity.vmm_overhead.measured_vcpu_milli),
    );
    capacity.cache_zone = value
        .get("cache_zone")
        .and_then(serde_json::Value::as_str)
        .map(CacheZone::new);
    if let Some(last_contact) = value
        .get("last_contact_ms")
        .and_then(serde_json::Value::as_u64)
    {
        capacity.last_contact = Tick::new(last_contact);
    }
}

pub(crate) fn load_state(connection: &Connection) -> Result<LedgerState, LedgerError> {
    migrate(connection)?;
    let mut state = LedgerState::new(Default::default());
    let mut hosts = connection
        .prepare(
            "SELECT host_key, incarnation, hostname, arch, total_vcpu, total_memory_gib,
                    total_disk_bytes, capabilities_json, admission_state, admission_reason,
                    last_contact_at
             FROM host ORDER BY id",
        )
        .map_err(|error| StoreError::Sqlite(error))?;
    let rows = hosts
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
            ))
        })
        .map_err(|error| StoreError::Sqlite(error))?;
    for row in rows {
        let (
            host_key,
            incarnation,
            hostname,
            arch,
            total_vcpu,
            total_memory_gib,
            total_disk_bytes,
            capabilities_json,
            admission_state,
            admission_reason,
            last_contact_at,
        ) = row.map_err(|error| StoreError::Sqlite(error))?;
        let arch = Arch::from_label_token(&arch).map_err(|error| {
            StoreError::Migration(format!("invalid host architecture: {error}"))
        })?;
        let mut capacity = HostCapacity {
            host_key: HostKey::new(host_key),
            incarnation: HostIncarnation::new(u64::try_from(incarnation).unwrap_or(0)),
            hostname,
            arch,
            total_vcpu: u32::try_from(total_vcpu).unwrap_or(0),
            total_memory_gib: u32::try_from(total_memory_gib).unwrap_or(0),
            total_disk_bytes: u64::try_from(total_disk_bytes).unwrap_or(0),
            headroom_vcpu: 0,
            headroom_memory_gib: 0,
            headroom_disk_bytes: 0,
            vmm_overhead: VmmOverhead::zero(),
            admission: if admission_state == "eligible" {
                HostAdmission::Eligible
            } else {
                HostAdmission::Ineligible {
                    reason: admission_reason.unwrap_or_else(|| admission_state.clone()),
                }
            },
            cache_zone: None,
            last_contact: parse_tick(last_contact_at),
        };
        apply_metadata(&mut capacity, &capabilities_json);
        state.restore_host(capacity);
    }
    drop(hosts);

    let mut reservations = connection
        .prepare(
            "SELECT r.id, r.incarnation, r.profile_revision_id, r.vcpu, r.memory_gib,
                    r.disk_bytes, r.state, r.created_at, h.host_key, h.arch
             FROM reservation r JOIN host h ON h.id = r.host_id
             WHERE r.state IN ('pending', 'reserved', 'idle_warm', 'busy')
             ORDER BY r.id",
        )
        .map_err(|error| StoreError::Sqlite(error))?;
    let rows = reservations
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })
        .map_err(|error| StoreError::Sqlite(error))?;
    for row in rows {
        let (
            id,
            incarnation,
            revision_id,
            vcpu,
            memory_gib,
            disk_bytes,
            state_text,
            created_at,
            host_key,
            arch,
        ) = row.map_err(|error| StoreError::Sqlite(error))?;
        let revision_id = revision_id.parse().map_err(|error: String| {
            StoreError::Migration(format!("invalid profile revision id: {error}"))
        })?;
        let arch = Arch::from_label_token(&arch).map_err(|error| {
            StoreError::Migration(format!("invalid reservation architecture: {error}"))
        })?;
        let shape = Shape::new(
            u16::try_from(vcpu).unwrap_or(u16::MAX),
            u32::try_from(memory_gib).unwrap_or(u32::MAX),
        )
        .map_err(|error| StoreError::Migration(format!("invalid reservation shape: {error}")))?;
        let reservation_state = ReservationState::parse(&state_text).ok_or_else(|| {
            StoreError::Migration(format!("unrecognized reservation state {state_text}"))
        })?;
        let sequence = u64::try_from(id).unwrap_or(0);
        state.restore_reservation(Reservation {
            id: crate::ledger::types::ReservationId::new(u64::try_from(id).unwrap_or(0)),
            sequence,
            enqueued_at: parse_tick(created_at),
            state: reservation_state,
            request: crate::ledger::types::ReservationRequest {
                revision_id,
                arch,
                shape,
                disk_bytes: u64::try_from(disk_bytes).unwrap_or(0),
                preferred_cache_zone: None,
            },
            host: Some(HostKey::new(host_key)),
            incarnation: HostIncarnation::new(u64::try_from(incarnation).unwrap_or(0)),
        });
    }

    Ok(state)
}

pub(crate) fn upsert_host(connection: &Connection, capacity: &HostCapacity) -> Result<(), LedgerError> {
    let admission_state = match &capacity.admission {
        HostAdmission::Eligible => "eligible",
        HostAdmission::Ineligible { .. } => "ineligible",
    };
    let admission_reason = match &capacity.admission {
        HostAdmission::Eligible => None,
        HostAdmission::Ineligible { reason } => Some(reason.clone()),
    };
    connection
        .execute(
            "INSERT INTO host (
                host_key, incarnation, hostname, arch, total_vcpu, total_memory_gib,
                total_disk_bytes, capabilities_json, admission_state, admission_reason,
                last_contact_at, drain_revision, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)
             ON CONFLICT(host_key) DO UPDATE SET
                incarnation = excluded.incarnation,
                hostname = excluded.hostname,
                arch = excluded.arch,
                total_vcpu = excluded.total_vcpu,
                total_memory_gib = excluded.total_memory_gib,
                total_disk_bytes = excluded.total_disk_bytes,
                capabilities_json = excluded.capabilities_json,
                admission_state = excluded.admission_state,
                admission_reason = excluded.admission_reason,
                last_contact_at = excluded.last_contact_at",
            params![
                capacity.host_key.as_str(),
                i64::try_from(capacity.incarnation.get()).unwrap_or(i64::MAX),
                capacity.hostname,
                capacity.arch.as_label_token(),
                i64::from(capacity.total_vcpu),
                i64::from(capacity.total_memory_gib),
                i64::try_from(capacity.total_disk_bytes).unwrap_or(i64::MAX),
                metadata(capacity),
                admission_state,
                admission_reason,
                tick_text(capacity.last_contact),
                tick_text(capacity.last_contact),
            ],
        )
        .map_err(|error| StoreError::Sqlite(error))?;
    Ok(())
}

pub(crate) fn delete_host(connection: &Connection, host_key: &HostKey) -> Result<(), LedgerError> {
    connection
        .execute("DELETE FROM host WHERE host_key = ?1", params![host_key.as_str()])
        .map_err(|error| StoreError::Sqlite(error))?;
    Ok(())
}

pub(crate) fn insert_reservation(
    connection: &Connection,
    reservation: &Reservation,
) -> Result<(), LedgerError> {
    let host_key = reservation
        .host
        .as_ref()
        .expect("a granted reservation has a host");
    let host_id: i64 = connection
        .query_row(
            "SELECT id FROM host WHERE host_key = ?1",
            params![host_key.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| StoreError::Sqlite(error))?;
    connection
        .execute(
            "INSERT INTO reservation (
                id, host_id, incarnation, profile_revision_id, vcpu, memory_gib,
                disk_bytes, state, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                i64::try_from(reservation.id.get()).unwrap_or(i64::MAX),
                host_id,
                i64::try_from(reservation.incarnation.get()).unwrap_or(i64::MAX),
                reservation.request.revision_id.to_hex(),
                i64::from(reservation.request.shape.vcpu().get()),
                i64::from(reservation.request.shape.memory_gib()),
                i64::try_from(reservation.request.disk_bytes).unwrap_or(i64::MAX),
                reservation.state.as_str(),
                tick_text(reservation.enqueued_at),
            ],
        )
        .map_err(|error| StoreError::Sqlite(error))?;
    Ok(())
}

pub(crate) fn set_reservation_state(
    connection: &Connection,
    id: crate::ledger::types::ReservationId,
    state: ReservationState,
    released_at: Option<Tick>,
) -> Result<(), LedgerError> {
    connection
        .execute(
            "UPDATE reservation SET state = ?1, released_at = ?2 WHERE id = ?3",
            params![
                state.as_str(),
                released_at.map(tick_text),
                i64::try_from(id.get()).unwrap_or(i64::MAX),
            ],
        )
        .map_err(|error| StoreError::Sqlite(error))?;
    Ok(())
}

pub(crate) fn update_heartbeat(
    connection: &Connection,
    host_key: &HostKey,
    incarnation: HostIncarnation,
    now: Tick,
) -> Result<(), LedgerError> {
    let changed = connection
        .execute(
            "UPDATE host SET last_contact_at = ?1 WHERE host_key = ?2 AND incarnation = ?3",
            params![
                tick_text(now),
                host_key.as_str(),
                i64::try_from(incarnation.get()).unwrap_or(i64::MAX),
            ],
        )
        .map_err(|error| StoreError::Sqlite(error))?;
    if changed == 0 {
        return Err(LedgerError::UnknownHost(host_key.clone()));
    }
    Ok(())
}
