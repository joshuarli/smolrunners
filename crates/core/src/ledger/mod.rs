mod engine;
mod error;
mod sqlite;
mod types;

#[cfg(test)]
mod tests;

use std::path::Path;

use rusqlite::Connection;

use crate::store::{StoreError, open_connection};
pub use engine::{Cycle, LedgerState, ReservationOutcome};
pub use error::{LedgerError, PendingReason, PendingStatus, UnschedulableReason};
pub use types::{
    CacheZone, CapacityOffer, DEFAULT_MAX_WAIT, DemandCost, HostAdmission, HostCapacity,
    HostIncarnation, HostKey, LedgerConfig, Reservation, ReservationId, ReservationRequest,
    ReservationState, ScaleSetOffer, Tick, Usage, VmmOverhead,
};

pub struct Ledger {
    state: LedgerState,
    connection: Option<Connection>,
}

impl Ledger {
    pub fn in_memory(config: LedgerConfig) -> Self {
        Self {
            state: LedgerState::new(config),
            connection: None,
        }
    }

    pub fn from_connection(connection: Connection) -> Result<Self, LedgerError> {
        let state = sqlite::load_state(&connection)?;
        Ok(Self {
            state,
            connection: Some(connection),
        })
    }

    pub fn open_path(path: impl AsRef<Path>) -> Result<Self, LedgerError> {
        let connection = open_connection(path)?;
        Self::from_connection(connection)
    }

    pub fn state(&self) -> &LedgerState {
        &self.state
    }

    pub fn connection(&self) -> Option<&Connection> {
        self.connection.as_ref()
    }

    pub fn into_connection(self) -> Option<Connection> {
        self.connection
    }

    pub fn host(&self, host_key: &HostKey) -> Option<&HostCapacity> {
        self.state.host(host_key)
    }

    pub fn hosts(&self) -> impl Iterator<Item = &HostCapacity> {
        self.state.hosts()
    }

    pub fn reservations(&self) -> impl Iterator<Item = &Reservation> {
        self.state.reservations()
    }

    pub fn reservation(&self, id: ReservationId) -> Option<&Reservation> {
        self.state.reservation(id)
    }

    pub fn status(&self, id: ReservationId, now: Tick) -> Option<PendingStatus> {
        self.state.status(id, now)
    }

    pub fn usage(&self, host_key: &HostKey) -> Usage {
        self.state.usage(host_key)
    }

    pub fn barrier(&self) -> Option<u64> {
        self.state.barrier()
    }

    pub fn expired_hosts(&self, now: Tick, lease_ms: u64) -> Vec<HostKey> {
        self.state.expired_hosts(now, lease_ms)
    }

    pub fn offers_for(&self, revision: &crate::domain::ProfileRevision) -> ScaleSetOffer {
        self.state.offers_for(revision)
    }

    pub fn register_host(&mut self, capacity: HostCapacity) -> Result<(), LedgerError> {
        self.state.register_host(capacity.clone())?;
        if let Some(connection) = &self.connection {
            sqlite::upsert_host(connection, &capacity)?;
        }
        Ok(())
    }

    pub fn deregister_host(
        &mut self,
        host_key: &HostKey,
        incarnation: HostIncarnation,
    ) -> Result<(), LedgerError> {
        self.state.deregister_host(host_key, incarnation)?;
        if let Some(connection) = &self.connection {
            sqlite::delete_host(connection, host_key)?;
        }
        Ok(())
    }

    pub fn submit(&mut self, request: ReservationRequest, now: Tick) -> ReservationId {
        self.state.submit(request, now)
    }

    pub fn reserve(
        &mut self,
        request: ReservationRequest,
        now: Tick,
    ) -> Result<ReservationOutcome, LedgerError> {
        let id = self.submit(request, now);
        self.plan(now)?;
        let status = self
            .status(id, now)
            .expect("a just-submitted reservation has a status");
        match status {
            PendingStatus::Granted { offer } => Ok(ReservationOutcome::Granted(offer)),
            status => Ok(ReservationOutcome::Pending { id, status }),
        }
    }

    pub fn plan(&mut self, now: Tick) -> Result<Cycle, LedgerError> {
        let cycle = self.state.plan(now);
        self.persist_cycle(&cycle)?;
        Ok(cycle)
    }

    pub fn mark_busy(
        &mut self,
        id: ReservationId,
        incarnation: HostIncarnation,
    ) -> Result<(), LedgerError> {
        self.state.mark_busy(id, incarnation)?;
        if let Some(connection) = &self.connection {
            sqlite::set_reservation_state(connection, id, ReservationState::Busy, None)?;
        }
        Ok(())
    }

    pub fn mark_idle_warm(
        &mut self,
        id: ReservationId,
        incarnation: HostIncarnation,
    ) -> Result<(), LedgerError> {
        self.state.mark_idle_warm(id, incarnation)?;
        if let Some(connection) = &self.connection {
            sqlite::set_reservation_state(connection, id, ReservationState::IdleWarm, None)?;
        }
        Ok(())
    }

    pub fn release(
        &mut self,
        id: ReservationId,
        host_key: &HostKey,
        incarnation: HostIncarnation,
        now: Tick,
    ) -> Result<(), LedgerError> {
        self.state.release(id, host_key, incarnation, now)?;
        if let Some(connection) = &self.connection {
            sqlite::set_reservation_state(
                connection,
                id,
                ReservationState::Released,
                Some(now),
            )?;
        }
        Ok(())
    }

    pub fn note_heartbeat(
        &mut self,
        host_key: &HostKey,
        incarnation: HostIncarnation,
        now: Tick,
    ) -> Result<(), LedgerError> {
        self.state.note_heartbeat(host_key, incarnation, now)?;
        if let Some(connection) = &self.connection {
            sqlite::update_heartbeat(connection, host_key, incarnation, now)?;
        }
        Ok(())
    }

    fn persist_cycle(&self, cycle: &Cycle) -> Result<(), LedgerError> {
        let Some(connection) = &self.connection else {
            return Ok(());
        };
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| StoreError::Sqlite(error))?;
        for offer in &cycle.offers {
            let reservation = self
                .state
                .reservation(offer.reservation_id)
                .expect("a granted offer has a reservation");
            sqlite::insert_reservation(&transaction, reservation)?;
        }
        for id in &cycle.reclaimed {
            sqlite::set_reservation_state(
                &transaction,
                *id,
                ReservationState::Released,
                None,
            )?;
        }
        transaction
            .commit()
            .map_err(|error| StoreError::Sqlite(error))?;
        Ok(())
    }
}
