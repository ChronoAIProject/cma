//! Test doubles shared by the service's test modules.
//!
//! The store and the adapter are the two things a session depends on that are
//! not the core itself, so faking exactly those lets the tests exercise real
//! service code. The store is deliberately shareable between service
//! instances: that is what makes "restart" expressible as a test.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use layer3_adapter::{AdapterError, KernelAdapter, TerminalInput, TerminalInputSender};
use tokio::sync::{mpsc, Mutex};

use crate::authz::{Authorization, Caller, Capability};
use crate::coordination::{
    RuntimeAuthority, RuntimeAuthorityRetirement, RuntimeAuthoritySwitch, RuntimeCandidate,
    RuntimeCleanup, RuntimeCleanupReason, RuntimeReservation, SessionBoundary,
    SessionInteractionLeaseRequest, SessionLease, SessionLeaseRequest, SessionOperation,
};
use crate::environment_service::EnvironmentProfileService;
use crate::event::{Event, StoredEvent};
use crate::ids::{KernelId, SessionId, SubjectId};
use crate::service::{ScreenFactory, SessionService};
use crate::session::SessionSpec;
use crate::store::{Page, PageCursor, SessionListQuery, SessionRecord, SessionStore, StoreError};
use crate::terminal::{ScreenModel, ScreenSnapshot, TerminalGeometry};
use crate::Reserved;

/// A store held in memory, shared between service instances by `Arc`.
///
/// Stands in for the database precisely where it matters: it is the only thing
/// two service instances have in common, so anything a restarted service can
/// see must have gone through it.
#[derive(Default)]
pub(crate) struct FakeStore {
    /// Makes multi-collection operations atomic to every fake-store reader.
    transaction: Mutex<()>,
    pub(crate) sessions: Mutex<HashMap<String, SessionRecord>>,
    pub(crate) events: Mutex<Vec<StoredEvent>>,
    /// Durable runtime authority, leases, and reservations shared by replicas.
    coordination: Mutex<HashMap<String, FakeCoordination>>,
    /// Durable receipts for whole record/event transactions, including empty batches.
    write_receipts: Mutex<HashMap<String, (SessionId, u64)>>,
    /// Successful record writes, so idempotent migrations can be asserted.
    pub(crate) upserts: std::sync::atomic::AtomicUsize,
    /// When set, every operation fails, so callers can be checked against a
    /// store that is down rather than merely empty.
    pub(crate) broken: Mutex<bool>,
    /// When set, durable history alone is unavailable.
    ///
    /// Reconciliation can still read the desired session record and the
    /// provider can still report its runtime alive. This isolates the case
    /// where readoption must defer rather than mistake an incomplete rebuild
    /// for a lost sandbox.
    pub(crate) broken_event_reads: Mutex<bool>,
    /// Monotonic stand-in for promotion after the exact old-writer-drain proof.
    legacy_cleanup_authorized: std::sync::atomic::AtomicBool,
}

#[derive(Debug, Clone)]
struct FakeLease {
    lease: SessionLease,
    interaction_nonce: Option<String>,
    expires_at: tokio::time::Instant,
}

#[derive(Debug, Clone)]
struct FakeReservation {
    reservation: RuntimeReservation,
    lease_token: String,
}

#[derive(Debug, Clone)]
struct FakeCandidate {
    candidate: RuntimeCandidate,
    lease_token: String,
}

#[derive(Debug, Clone)]
struct FakeSwitch {
    candidate: RuntimeCandidate,
    lease_token: String,
    outcome: RuntimeAuthoritySwitch,
}

#[derive(Debug, Clone)]
struct FakeRetirement {
    lease_token: String,
    original_boundary: SessionBoundary,
    outcome: RuntimeAuthorityRetirement,
}

#[derive(Debug, Clone)]
struct FakeCoordination {
    authority: RuntimeAuthority,
    lease: Option<FakeLease>,
    retired_lease_tokens: HashSet<String>,
    interaction_epoch: u64,
    next_generation: u64,
    reservations: HashMap<String, FakeReservation>,
    candidates: HashMap<String, FakeCandidate>,
    cleanups: HashMap<String, RuntimeCleanup>,
    completed_cleanups: HashMap<String, RuntimeCleanup>,
    switches: HashMap<String, FakeSwitch>,
    retirements: HashMap<String, FakeRetirement>,
}

impl FakeCoordination {
    fn legacy(record: &SessionRecord) -> Self {
        Self {
            authority: RuntimeAuthority {
                generation: 0,
                runtime_id: record.runtime_id.clone(),
            },
            lease: None,
            retired_lease_tokens: HashSet::new(),
            interaction_epoch: 0,
            next_generation: 0,
            reservations: HashMap::new(),
            candidates: HashMap::new(),
            cleanups: HashMap::new(),
            completed_cleanups: HashMap::new(),
            switches: HashMap::new(),
            retirements: HashMap::new(),
        }
    }

    fn expire_lease(&mut self, now: tokio::time::Instant) {
        if self
            .lease
            .as_ref()
            .is_some_and(|held| held.expires_at <= now)
        {
            let expired = self.lease.take().expect("the expired lease was present");
            if expired.lease.operation != SessionOperation::Interact {
                self.retired_lease_tokens.insert(expired.lease.token);
            }
        }
    }
}

impl FakeStore {
    /// Install an exact record for a fixture, outside the production store port.
    pub(crate) async fn seed(&self, record: &SessionRecord) -> Result<(), StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        self.upserts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.sessions
            .lock()
            .await
            .insert(record.session.id.0.clone(), record.clone());
        self.coordination.lock().await.insert(
            record.session.id.0.clone(),
            FakeCoordination::legacy(record),
        );
        Ok(())
    }

    /// The exact lease currently held by a fixture session, if any.
    pub(crate) async fn active_session_lease(&self, id: &SessionId) -> Option<SessionLease> {
        self.coordination
            .lock()
            .await
            .get(&id.0)
            .and_then(|state| state.lease.as_ref())
            .map(|held| held.lease.clone())
    }

    pub(crate) async fn fail_everything(&self) {
        *self.broken.lock().await = true;
    }

    /// Model the one-way promotion that permanently fences legacy writers.
    pub(crate) fn authorize_legacy_cleanup(&self) {
        self.legacy_cleanup_authorized
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    async fn guard(&self) -> Result<(), StoreError> {
        if *self.broken.lock().await {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        Ok(())
    }

    fn event_tail(held: &[StoredEvent], id: &SessionId) -> u64 {
        held.iter()
            .filter(|event| &event.session_id == id)
            .map(|event| event.seq)
            .max()
            .unwrap_or(0)
    }

    fn validate_append(
        held: &[StoredEvent],
        events: &[Event],
        recorded_tail: u64,
    ) -> Result<(), StoreError> {
        let Some(first) = events.first() else {
            return Ok(());
        };
        let id = &first.session_id;
        let mut expected = Self::event_tail(held, id)
            .max(recorded_tail)
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed(format!("event sequence overflow for {id}")))?;

        for event in events {
            if event.session_id != *id {
                return Err(StoreError::Malformed(
                    "one event append batch named more than one session".into(),
                ));
            }
            if event.seq < expected {
                return Err(StoreError::SequenceTaken {
                    expected,
                    got: event.seq,
                });
            }
            if event.seq > expected {
                return Err(StoreError::NonContiguousAppend {
                    expected,
                    got: event.seq,
                });
            }
            expected = expected.checked_add(1).ok_or_else(|| {
                StoreError::Malformed(format!("event sequence overflow for {id}"))
            })?;
        }
        Ok(())
    }

    async fn commit_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
        create: bool,
    ) -> Result<u64, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let mut receipts = self.write_receipts.lock().await;
        if let Some((held_by, revision)) = receipts.get(write_id) {
            if held_by != &record.session.id {
                return Err(StoreError::Malformed(format!(
                    "session write ID {write_id} belongs to another session"
                )));
            }
            return Ok(*revision);
        }

        let mut sessions = self.sessions.lock().await;
        let existing = sessions.get(&record.session.id.0);
        let actual = existing.map_or(0, |stored| stored.revision);
        let recorded_tail = existing.map_or(0, |stored| stored.session.last_seq);
        if create {
            if record.revision != 0 || existing.is_some() {
                return Err(StoreError::RevisionConflict {
                    expected: 0,
                    actual,
                });
            }
        } else {
            if existing.is_none() {
                return Err(StoreError::SessionDeleted {
                    id: record.session.id.clone(),
                });
            }
            if actual != record.revision {
                return Err(StoreError::RevisionConflict {
                    expected: record.revision,
                    actual,
                });
            }
            if existing.is_some_and(|stored| {
                stored.session.status.is_terminal() && !record.session.status.is_terminal()
            }) {
                return Err(StoreError::SessionTerminated {
                    id: record.session.id.clone(),
                });
            }
        }

        let mut held = self.events.lock().await;
        let sequence_floor = Self::event_tail(&held, &record.session.id).max(recorded_tail);
        Self::validate_append(&held, events, recorded_tail)?;
        let requested_tail = record.session.last_seq;
        let durable_tail = events.last().map_or(sequence_floor, |event| event.seq);
        let tail_matches = if events.is_empty() {
            requested_tail <= sequence_floor
        } else {
            requested_tail == durable_tail
        };
        if !tail_matches {
            return Err(StoreError::Malformed(format!(
                "session {} record tail {} does not match durable tail {}",
                record.session.id, requested_tail, durable_tail
            )));
        }
        let revision = actual
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("session revision overflow".into()))?;
        let mut durable = record.clone();
        durable.revision = revision;
        durable.session.last_seq = durable_tail;
        held.extend(events.iter().cloned().map(StoredEvent::from));
        sessions.insert(record.session.id.0.clone(), durable.clone());
        let mut coordination = self.coordination.lock().await;
        let state = coordination
            .entry(record.session.id.0.clone())
            .or_insert_with(|| FakeCoordination::legacy(&durable));
        if state.authority.generation == 0 && state.lease.is_none() {
            // Compatibility-stage writers have no separate authority switch.
            // Once a recorded generation exists, ordinary record/event commits
            // must never be allowed to move runtime ownership.
            state.authority.runtime_id = durable.runtime_id.clone();
        }
        receipts.insert(write_id.to_string(), (record.session.id.clone(), revision));
        self.upserts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(revision)
    }
}

#[async_trait::async_trait]
impl SessionStore for FakeStore {
    async fn ping(&self) -> Result<(), StoreError> {
        self.guard().await
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        Ok(self.sessions.lock().await.get(&id.0).cloned())
    }

    async fn runtime_authority(
        &self,
        id: &SessionId,
    ) -> Result<Option<RuntimeAuthority>, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let Some(record) = self.sessions.lock().await.get(&id.0).cloned() else {
            return Ok(None);
        };
        let mut coordination = self.coordination.lock().await;
        let state = coordination
            .entry(id.0.clone())
            .or_insert_with(|| FakeCoordination::legacy(&record));
        if state.authority.runtime_id != record.runtime_id {
            return Err(StoreError::Malformed(format!(
                "session {id} record and runtime authority disagree"
            )));
        }
        Ok(Some(state.authority.clone()))
    }

    async fn acquire_session_lease(
        &self,
        request: &SessionLeaseRequest,
    ) -> Result<SessionLease, StoreError> {
        if request.operation == SessionOperation::Interact {
            return Err(StoreError::Malformed(
                "interaction leases require bounded interaction acquisition".into(),
            ));
        }
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let Some(record) = self
            .sessions
            .lock()
            .await
            .get(&request.session_id.0)
            .cloned()
        else {
            return Err(StoreError::SessionDeleted {
                id: request.session_id.clone(),
            });
        };
        let mut coordination = self.coordination.lock().await;
        let state = coordination
            .entry(request.session_id.0.clone())
            .or_insert_with(|| FakeCoordination::legacy(&record));
        let current = SessionBoundary::for_record(&record, state.authority.generation);
        if state.authority.runtime_id != record.runtime_id || current != request.boundary {
            return Err(StoreError::SessionAuthorityChanged {
                id: request.session_id.clone(),
            });
        }

        let now = tokio::time::Instant::now();
        state.expire_lease(now);
        let lease = SessionLease {
            session_id: request.session_id.clone(),
            holder: request.holder.clone(),
            token: request.token.clone(),
            boundary: request.boundary.clone(),
            operation: request.operation,
        };
        if let Some(held) = state.lease.as_mut() {
            if held.lease != lease {
                return Err(StoreError::SessionLeaseHeld {
                    id: request.session_id.clone(),
                });
            }
            held.expires_at = now
                .checked_add(request.ttl)
                .ok_or_else(|| StoreError::Malformed("session lease expiry overflow".into()))?;
            return Ok(held.lease.clone());
        }
        if state.retired_lease_tokens.contains(&lease.token) {
            return Err(StoreError::SessionLeaseLost {
                id: request.session_id.clone(),
            });
        }
        state.lease = Some(FakeLease {
            lease: lease.clone(),
            interaction_nonce: None,
            expires_at: now
                .checked_add(request.ttl)
                .ok_or_else(|| StoreError::Malformed("session lease expiry overflow".into()))?,
        });
        Ok(lease)
    }

    async fn acquire_interaction_lease(
        &self,
        request: &SessionInteractionLeaseRequest,
    ) -> Result<SessionLease, StoreError> {
        if request.holder.is_empty() || request.nonce.is_empty() {
            return Err(StoreError::Malformed(
                "session interaction lease holder and nonce must not be empty".into(),
            ));
        }
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let Some(record) = self
            .sessions
            .lock()
            .await
            .get(&request.session_id.0)
            .cloned()
        else {
            return Err(StoreError::SessionDeleted {
                id: request.session_id.clone(),
            });
        };
        let mut coordination = self.coordination.lock().await;
        let state = coordination
            .entry(request.session_id.0.clone())
            .or_insert_with(|| FakeCoordination::legacy(&record));
        let current = SessionBoundary::for_record(&record, state.authority.generation);
        if state.authority.runtime_id != record.runtime_id || current != request.boundary {
            return Err(StoreError::SessionAuthorityChanged {
                id: request.session_id.clone(),
            });
        }

        let now = tokio::time::Instant::now();
        state.expire_lease(now);
        if let Some(held) = state.lease.as_mut() {
            if held.interaction_nonce.as_deref() == Some(request.nonce.as_str())
                && held.lease.holder == request.holder
                && held.lease.boundary == request.boundary
                && held.lease.operation == SessionOperation::Interact
            {
                held.expires_at = now
                    .checked_add(request.ttl)
                    .ok_or_else(|| StoreError::Malformed("session lease expiry overflow".into()))?;
                return Ok(held.lease.clone());
            }
            return Err(StoreError::SessionInteractionBlocked {
                id: request.session_id.clone(),
                operation: held.lease.operation,
            });
        }
        state.interaction_epoch = state
            .interaction_epoch
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("interaction lease epoch overflow".into()))?;
        let lease = SessionLease {
            session_id: request.session_id.clone(),
            holder: request.holder.clone(),
            token: format!("interaction:{}", state.interaction_epoch),
            boundary: request.boundary.clone(),
            operation: SessionOperation::Interact,
        };
        state.lease = Some(FakeLease {
            lease: lease.clone(),
            interaction_nonce: Some(request.nonce.clone()),
            expires_at: now
                .checked_add(request.ttl)
                .ok_or_else(|| StoreError::Malformed("session lease expiry overflow".into()))?,
        });
        Ok(lease)
    }

    async fn renew_session_lease(
        &self,
        lease: &SessionLease,
        ttl: std::time::Duration,
    ) -> Result<(), StoreError> {
        if lease.operation == SessionOperation::Interact {
            return Err(StoreError::Malformed(
                "interaction leases require interaction renewal".into(),
            ));
        }
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let Some(record) = self.sessions.lock().await.get(&lease.session_id.0).cloned() else {
            return Err(StoreError::SessionDeleted {
                id: lease.session_id.clone(),
            });
        };
        let mut coordination = self.coordination.lock().await;
        let Some(state) = coordination.get_mut(&lease.session_id.0) else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        let now = tokio::time::Instant::now();
        state.expire_lease(now);
        let current = SessionBoundary::for_record(&record, state.authority.generation);
        let Some(held) = state.lease.as_mut() else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        if held.lease != *lease || current != lease.boundary {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        }
        held.expires_at = now
            .checked_add(ttl)
            .ok_or_else(|| StoreError::Malformed("session lease expiry overflow".into()))?;
        Ok(())
    }

    async fn renew_interaction_lease(
        &self,
        lease: &SessionLease,
        ttl: std::time::Duration,
    ) -> Result<(), StoreError> {
        if lease.operation != SessionOperation::Interact {
            return Err(StoreError::Malformed(
                "provider-operation leases require provider renewal".into(),
            ));
        }
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let Some(record) = self.sessions.lock().await.get(&lease.session_id.0).cloned() else {
            return Err(StoreError::SessionDeleted {
                id: lease.session_id.clone(),
            });
        };
        let mut coordination = self.coordination.lock().await;
        let Some(state) = coordination.get_mut(&lease.session_id.0) else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        let now = tokio::time::Instant::now();
        state.expire_lease(now);
        let current = SessionBoundary::for_record(&record, state.authority.generation);
        let Some(held) = state.lease.as_mut() else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        if held.lease != *lease
            || current.authority != lease.boundary.authority
            || state.authority.runtime_id != record.runtime_id
        {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        }
        held.expires_at = now
            .checked_add(ttl)
            .ok_or_else(|| StoreError::Malformed("session lease expiry overflow".into()))?;
        Ok(())
    }

    async fn advance_session_lease(
        &self,
        lease: &mut SessionLease,
        boundary: &SessionBoundary,
    ) -> Result<(), StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let Some(record) = self.sessions.lock().await.get(&lease.session_id.0).cloned() else {
            return Err(StoreError::SessionDeleted {
                id: lease.session_id.clone(),
            });
        };
        let mut coordination = self.coordination.lock().await;
        let Some(state) = coordination.get_mut(&lease.session_id.0) else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        let now = tokio::time::Instant::now();
        state.expire_lease(now);
        let current = SessionBoundary::for_record(&record, state.authority.generation);
        let Some(held) = state.lease.as_mut() else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        if held.lease != *lease || current != *boundary {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        }
        held.lease.boundary = boundary.clone();
        lease.boundary = boundary.clone();
        Ok(())
    }

    async fn release_session_lease(&self, lease: &SessionLease) -> Result<(), StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let Some(record) = self.sessions.lock().await.get(&lease.session_id.0).cloned() else {
            return Err(StoreError::SessionDeleted {
                id: lease.session_id.clone(),
            });
        };
        let mut coordination = self.coordination.lock().await;
        let Some(state) = coordination.get_mut(&lease.session_id.0) else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        let now = tokio::time::Instant::now();
        state.expire_lease(now);
        let current = SessionBoundary::for_record(&record, state.authority.generation);
        if lease.operation == SessionOperation::Interact {
            let lease_epoch = lease
                .token
                .strip_prefix("interaction:")
                .and_then(|epoch| epoch.parse::<u64>().ok())
                .filter(|epoch| *epoch != 0 && *epoch <= state.interaction_epoch)
                .ok_or_else(|| StoreError::SessionLeaseLost {
                    id: lease.session_id.clone(),
                })?;
            match state.lease.as_ref() {
                Some(held)
                    if held.lease.operation == SessionOperation::Interact
                        && lease_epoch == state.interaction_epoch =>
                {
                    if held.lease != *lease {
                        return Err(StoreError::SessionLeaseLost {
                            id: lease.session_id.clone(),
                        });
                    }
                    state.lease = None;
                }
                // An older interaction epoch can never name the active lease.
                // With no active interaction, release is an idempotent no-op.
                _ => {}
            }
            return Ok(());
        }
        if state.lease.as_ref().map(|held| &held.lease) != Some(lease) || current != lease.boundary
        {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        }
        state.lease = None;
        state.retired_lease_tokens.insert(lease.token.clone());
        Ok(())
    }

    async fn reserve_runtime_generation(
        &self,
        lease: &SessionLease,
        reservation_token: &str,
    ) -> Result<RuntimeReservation, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let Some(record) = self.sessions.lock().await.get(&lease.session_id.0).cloned() else {
            return Err(StoreError::SessionDeleted {
                id: lease.session_id.clone(),
            });
        };
        let mut coordination = self.coordination.lock().await;
        let Some(state) = coordination.get_mut(&lease.session_id.0) else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        let now = tokio::time::Instant::now();
        state.expire_lease(now);
        let current = SessionBoundary::for_record(&record, state.authority.generation);
        if state.lease.as_ref().map(|held| &held.lease) != Some(lease) || current != lease.boundary
        {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        }
        if let Some(existing) = state.reservations.get(reservation_token) {
            if existing.lease_token == lease.token {
                return Ok(existing.reservation.clone());
            }
            return Err(StoreError::RuntimeReservationChanged {
                id: lease.session_id.clone(),
            });
        }
        let generation = state
            .next_generation
            .max(state.authority.generation)
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("runtime generation overflow".into()))?;
        state.next_generation = generation;
        let reservation = RuntimeReservation {
            session_id: lease.session_id.clone(),
            generation: std::num::NonZeroU64::new(generation).ok_or_else(|| {
                StoreError::Malformed("runtime generation must be non-zero".into())
            })?,
            token: reservation_token.to_string(),
        };
        state.reservations.insert(
            reservation_token.to_string(),
            FakeReservation {
                reservation: reservation.clone(),
                lease_token: lease.token.clone(),
            },
        );
        Ok(reservation)
    }

    async fn register_runtime_candidate(
        &self,
        lease: &SessionLease,
        reservation: &RuntimeReservation,
        runtime_id: &str,
    ) -> Result<RuntimeCandidate, StoreError> {
        self.guard().await?;
        if runtime_id.is_empty() {
            return Err(StoreError::Malformed(
                "runtime candidate handle must not be empty".into(),
            ));
        }
        let _transaction = self.transaction.lock().await;
        let Some(record) = self.sessions.lock().await.get(&lease.session_id.0).cloned() else {
            return Err(StoreError::SessionDeleted {
                id: lease.session_id.clone(),
            });
        };
        let mut coordination = self.coordination.lock().await;
        let Some(state) = coordination.get_mut(&lease.session_id.0) else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        let now = tokio::time::Instant::now();
        state.expire_lease(now);
        let current = SessionBoundary::for_record(&record, state.authority.generation);
        if state.lease.as_ref().map(|held| &held.lease) != Some(lease) || current != lease.boundary
        {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        }
        if reservation.session_id != lease.session_id {
            return Err(StoreError::RuntimeReservationChanged {
                id: lease.session_id.clone(),
            });
        }
        let Some(stored) = state.reservations.get(&reservation.token) else {
            return Err(StoreError::RuntimeReservationChanged {
                id: lease.session_id.clone(),
            });
        };
        if stored.lease_token != lease.token || stored.reservation != *reservation {
            return Err(StoreError::RuntimeReservationChanged {
                id: lease.session_id.clone(),
            });
        }
        let candidate = RuntimeCandidate {
            session_id: lease.session_id.clone(),
            generation: reservation.generation,
            runtime_id: runtime_id.to_string(),
            token: reservation.token.clone(),
        };
        if let Some(existing) = state.candidates.get(&reservation.token) {
            if existing.lease_token == lease.token && existing.candidate == candidate {
                return Ok(existing.candidate.clone());
            }
            return Err(StoreError::RuntimeCandidateChanged {
                id: lease.session_id.clone(),
            });
        }
        if state.authority.runtime_id.as_deref() == Some(runtime_id) {
            return Err(StoreError::RuntimeCandidateChanged {
                id: lease.session_id.clone(),
            });
        }

        let cleanup = RuntimeCleanup {
            session_id: lease.session_id.clone(),
            generation: candidate.generation.get(),
            runtime_id: candidate.runtime_id.clone(),
            token: format!("candidate:{}", candidate.token),
            reason: RuntimeCleanupReason::UncommittedCandidate,
        };
        state.cleanups.insert(cleanup.token.clone(), cleanup);
        state.candidates.insert(
            reservation.token.clone(),
            FakeCandidate {
                candidate: candidate.clone(),
                lease_token: lease.token.clone(),
            },
        );
        Ok(candidate)
    }

    async fn switch_runtime_authority(
        &self,
        lease: &mut SessionLease,
        candidate: &RuntimeCandidate,
    ) -> Result<RuntimeAuthoritySwitch, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let mut sessions = self.sessions.lock().await;
        let Some(record) = sessions.get_mut(&lease.session_id.0) else {
            return Err(StoreError::SessionDeleted {
                id: lease.session_id.clone(),
            });
        };
        let mut coordination = self.coordination.lock().await;
        let Some(state) = coordination.get_mut(&lease.session_id.0) else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        let now = tokio::time::Instant::now();
        state.expire_lease(now);

        let held_by_this_token = state
            .lease
            .as_ref()
            .is_some_and(|held| held.lease.token == lease.token);
        if let Some(completed) = state.switches.get(&candidate.token) {
            let current = SessionBoundary::for_record(record, state.authority.generation);
            let held_at_completed_boundary = state
                .lease
                .as_ref()
                .is_some_and(|held| held.lease.boundary == completed.outcome.boundary);
            if held_by_this_token
                && held_at_completed_boundary
                && completed.lease_token == lease.token
                && completed.candidate == *candidate
                && current == completed.outcome.boundary
            {
                lease.boundary = completed.outcome.boundary.clone();
                return Ok(completed.outcome.clone());
            }
            return Err(StoreError::RuntimeCandidateChanged {
                id: lease.session_id.clone(),
            });
        }

        let current = SessionBoundary::for_record(record, state.authority.generation);
        if state.lease.as_ref().map(|held| &held.lease) != Some(lease) || current != lease.boundary
        {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        }
        let Some(stored_candidate) = state.candidates.get(&candidate.token).cloned() else {
            return Err(StoreError::RuntimeCandidateChanged {
                id: lease.session_id.clone(),
            });
        };
        if stored_candidate.lease_token != lease.token
            || stored_candidate.candidate != *candidate
            || candidate.session_id != lease.session_id
            || candidate.generation.get() <= state.authority.generation
        {
            return Err(StoreError::RuntimeCandidateChanged {
                id: lease.session_id.clone(),
            });
        }

        let previous = state.authority.clone();
        let cleanup = previous
            .runtime_id
            .as_ref()
            .map(|runtime_id| RuntimeCleanup {
                session_id: lease.session_id.clone(),
                generation: previous.generation,
                runtime_id: runtime_id.clone(),
                token: format!("retired:{}", candidate.token),
                reason: RuntimeCleanupReason::RetiredAuthority,
            });
        let revision = record
            .revision
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("session revision overflow".into()))?;
        record.revision = revision;
        record.runtime_id = Some(candidate.runtime_id.clone());
        state.authority = RuntimeAuthority {
            generation: candidate.generation.get(),
            runtime_id: Some(candidate.runtime_id.clone()),
        };
        state.next_generation = state.next_generation.max(candidate.generation.get());
        state
            .cleanups
            .remove(&format!("candidate:{}", candidate.token));
        if let Some(cleanup) = cleanup.as_ref() {
            state
                .cleanups
                .insert(cleanup.token.clone(), cleanup.clone());
        }
        let boundary = SessionBoundary {
            status: record.session.status,
            authority: state.authority.clone(),
        };
        let Some(held) = state.lease.as_mut() else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        held.lease.boundary = boundary.clone();
        lease.boundary = boundary.clone();
        let outcome = RuntimeAuthoritySwitch {
            revision,
            boundary,
            cleanup,
        };
        state.switches.insert(
            candidate.token.clone(),
            FakeSwitch {
                candidate: candidate.clone(),
                lease_token: lease.token.clone(),
                outcome: outcome.clone(),
            },
        );
        Ok(outcome)
    }

    async fn retire_runtime_authority(
        &self,
        lease: &mut SessionLease,
        retirement_token: &str,
    ) -> Result<RuntimeAuthorityRetirement, StoreError> {
        self.guard().await?;
        if retirement_token.is_empty() {
            return Err(StoreError::Malformed(
                "runtime retirement token must not be empty".into(),
            ));
        }
        let _transaction = self.transaction.lock().await;
        let mut sessions = self.sessions.lock().await;
        let Some(record) = sessions.get_mut(&lease.session_id.0) else {
            return Err(StoreError::SessionDeleted {
                id: lease.session_id.clone(),
            });
        };
        let mut coordination = self.coordination.lock().await;
        let Some(state) = coordination.get_mut(&lease.session_id.0) else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        let now = tokio::time::Instant::now();
        state.expire_lease(now);

        let held_by_this_token = state
            .lease
            .as_ref()
            .is_some_and(|held| held.lease.token == lease.token);
        if let Some(completed) = state.retirements.get(retirement_token) {
            let current = SessionBoundary::for_record(record, state.authority.generation);
            let held_at_completed_boundary = state
                .lease
                .as_ref()
                .is_some_and(|held| held.lease.boundary == completed.outcome.boundary);
            if held_by_this_token
                && held_at_completed_boundary
                && completed.lease_token == lease.token
                && (completed.original_boundary == lease.boundary
                    || completed.outcome.boundary == lease.boundary)
                && current == completed.outcome.boundary
            {
                lease.boundary = completed.outcome.boundary.clone();
                return Ok(completed.outcome.clone());
            }
            return Err(StoreError::RuntimeCleanupChanged {
                id: lease.session_id.clone(),
            });
        }

        let current = SessionBoundary::for_record(record, state.authority.generation);
        if !matches!(
            lease.operation,
            SessionOperation::Recover | SessionOperation::Lifecycle
        ) || state.lease.as_ref().map(|held| &held.lease) != Some(lease)
            || current != lease.boundary
        {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        }
        let Some(runtime_id) = state.authority.runtime_id.clone() else {
            return Err(StoreError::RuntimeCleanupChanged {
                id: lease.session_id.clone(),
            });
        };
        let original_boundary = lease.boundary.clone();
        let cleanup = RuntimeCleanup {
            session_id: lease.session_id.clone(),
            generation: state.authority.generation,
            runtime_id,
            token: format!("retired:{retirement_token}"),
            reason: RuntimeCleanupReason::RetiredAuthority,
        };
        if state.cleanups.contains_key(&cleanup.token) {
            return Err(StoreError::RuntimeCleanupChanged {
                id: lease.session_id.clone(),
            });
        }
        let revision = record
            .revision
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("session revision overflow".into()))?;
        record.revision = revision;
        record.runtime_id = None;
        state.authority.runtime_id = None;
        state
            .cleanups
            .insert(cleanup.token.clone(), cleanup.clone());
        let boundary = SessionBoundary {
            status: record.session.status,
            authority: state.authority.clone(),
        };
        let Some(held) = state.lease.as_mut() else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        held.lease.boundary = boundary.clone();
        lease.boundary = boundary.clone();
        let outcome = RuntimeAuthorityRetirement {
            revision,
            boundary,
            cleanup,
        };
        state.retirements.insert(
            retirement_token.to_string(),
            FakeRetirement {
                lease_token: lease.token.clone(),
                original_boundary,
                outcome: outcome.clone(),
            },
        );
        Ok(outcome)
    }

    async fn runtime_cleanup_debts(
        &self,
        id: &SessionId,
    ) -> Result<Vec<RuntimeCleanup>, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let Some(state) = self.coordination.lock().await.get(&id.0).cloned() else {
            return Ok(Vec::new());
        };
        let mut cleanups = state.cleanups.into_values().collect::<Vec<_>>();
        cleanups.sort_by(|a, b| a.token.cmp(&b.token));
        Ok(cleanups)
    }

    async fn legacy_runtime_cleanup_authorized(&self) -> Result<bool, StoreError> {
        self.guard().await?;
        Ok(self
            .legacy_cleanup_authorized
            .load(std::sync::atomic::Ordering::SeqCst))
    }

    async fn acknowledge_runtime_cleanup(
        &self,
        lease: &SessionLease,
        cleanup: &RuntimeCleanup,
    ) -> Result<(), StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let Some(record) = self.sessions.lock().await.get(&lease.session_id.0).cloned() else {
            return Err(StoreError::SessionDeleted {
                id: lease.session_id.clone(),
            });
        };
        let mut coordination = self.coordination.lock().await;
        let Some(state) = coordination.get_mut(&lease.session_id.0) else {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        };
        let now = tokio::time::Instant::now();
        state.expire_lease(now);
        let current = SessionBoundary::for_record(&record, state.authority.generation);
        if !matches!(
            lease.operation,
            SessionOperation::Reclaim | SessionOperation::Recover | SessionOperation::Lifecycle
        ) || cleanup.session_id != lease.session_id
            || state.lease.as_ref().map(|held| &held.lease) != Some(lease)
            || current != lease.boundary
        {
            return Err(StoreError::SessionLeaseLost {
                id: lease.session_id.clone(),
            });
        }
        if state.authority.generation == cleanup.generation
            && state.authority.runtime_id.as_deref() == Some(cleanup.runtime_id.as_str())
        {
            return Err(StoreError::RuntimeCleanupChanged {
                id: lease.session_id.clone(),
            });
        }
        if let Some(completed) = state.completed_cleanups.get(&cleanup.token) {
            return if completed == cleanup {
                Ok(())
            } else {
                Err(StoreError::RuntimeCleanupChanged {
                    id: lease.session_id.clone(),
                })
            };
        }
        if state.cleanups.get(&cleanup.token) != Some(cleanup) {
            return Err(StoreError::RuntimeCleanupChanged {
                id: lease.session_id.clone(),
            });
        }
        let completed = state
            .cleanups
            .remove(&cleanup.token)
            .expect("the exact cleanup was present");
        state
            .completed_cleanups
            .insert(cleanup.token.clone(), completed);
        Ok(())
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        Ok(page_sessions(
            self.sessions
                .lock()
                .await
                .values()
                .filter(|r| &r.session.owner == owner)
                .cloned()
                .collect(),
            query,
        ))
    }

    async fn list_all(&self, query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        Ok(page_sessions(
            self.sessions.lock().await.values().cloned().collect(),
            query,
        ))
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let mut reserved = Reserved::default();
        for record in self.sessions.lock().await.values() {
            if record.session.wants_runtime() {
                reserved.include(record.session.options.size);
            }
        }
        Ok(reserved)
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let mut coordination = self.coordination.lock().await;
        if let Some(state) = coordination.get_mut(&id.0) {
            state.expire_lease(tokio::time::Instant::now());
            if state.lease.is_some() {
                return Err(StoreError::SessionLeaseHeld { id: id.clone() });
            }
            if (state.authority.generation != 0 && state.authority.runtime_id.is_some())
                || !state.cleanups.is_empty()
            {
                return Err(StoreError::SessionCleanupPending {
                    id: id.clone(),
                    runtime_id: state.authority.runtime_id.clone(),
                    cleanup_debts: state.cleanups.len(),
                });
            }
        }
        self.sessions.lock().await.remove(&id.0);
        self.events.lock().await.retain(|e| &e.session_id != id);
        coordination.remove(&id.0);
        self.write_receipts
            .lock()
            .await
            .retain(|_, (session_id, _)| session_id != id);
        Ok(())
    }

    /// Append atomically, refusing a collision or a forward gap.
    ///
    /// The refusal is the point. This double accepted duplicates, so a race
    /// that the database rejects outright looked perfectly healthy here — the
    /// suite could not see the defect that dropped a turn's events in
    /// production, because the only thing that would have caught it was the
    /// unique index this stands in for. A double that is more permissive than
    /// the thing it doubles is a test that passes for the wrong reason.
    ///
    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let recorded_tail = match events.first() {
            Some(first) => self
                .sessions
                .lock()
                .await
                .get(&first.session_id.0)
                .map_or(0, |record| record.session.last_seq),
            None => 0,
        };
        let mut held = self.events.lock().await;
        Self::validate_append(&held, events, recorded_tail)?;
        held.extend(events.iter().cloned().map(StoredEvent::from));
        Ok(())
    }

    async fn commit_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.commit_idempotent(write_id, record, events, false)
            .await
    }

    async fn create_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.commit_idempotent(write_id, record, events, true).await
    }

    async fn event_tail(&self, id: &SessionId) -> Result<u64, StoreError> {
        self.guard().await?;
        Ok(self
            .events
            .lock()
            .await
            .iter()
            .filter(|event| &event.session_id == id)
            .map(|event| event.seq)
            .max()
            .unwrap_or(0))
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.guard().await?;
        if *self.broken_event_reads.lock().await {
            return Err(StoreError::Unavailable("test event history is down".into()));
        }
        let mut out: Vec<StoredEvent> = self
            .events
            .lock()
            .await
            .iter()
            .filter(|e| &e.session_id == id && e.seq > after)
            .cloned()
            .collect();
        out.sort_by_key(|e| e.seq);
        Ok(out)
    }
}

fn page_sessions(mut records: Vec<SessionRecord>, query: &SessionListQuery) -> Page<SessionRecord> {
    records.retain(|record| {
        query
            .status
            .is_none_or(|status| record.session.status == status)
            && query.page.cursor().is_none_or(|cursor| {
                record.session.created_at < cursor.created_at
                    || (record.session.created_at == cursor.created_at
                        && record.session.id.0 < cursor.id)
            })
    });
    records.sort_by(|a, b| {
        b.session
            .created_at
            .cmp(&a.session.created_at)
            .then_with(|| b.session.id.0.cmp(&a.session.id.0))
    });
    let has_more = records.len() > query.page.limit() as usize;
    records.truncate(query.page.limit() as usize);
    let next = has_more.then(|| {
        let last = records
            .last()
            .expect("a non-empty bounded page has a last row");
        PageCursor::new(&last.session.created_at, &last.session.id.0)
    });
    Page {
        items: records,
        next,
    }
}

pub(crate) use crate::adapter_support::FakeAdapter;

pub(crate) fn terminal_input_channel(
    capacity: usize,
) -> (TerminalInputSender, mpsc::Receiver<TerminalInput>) {
    let (delivered, received) = mpsc::channel(capacity);
    let input = TerminalInputSender::spawn(capacity, move |input| {
        let delivered = delivered.clone();
        async move {
            delivered.send(input).await.map_err(|_| {
                AdapterError::Unavailable("the fake terminal runtime has ended".into())
            })
        }
    });
    (input, received)
}
pub(crate) use crate::environment_support::{
    baseline, catalog_service, environments, fake_agent, profile_spec, unconfigured_settings,
    FakeProfileStore, FakeSkillCatalog, ReplacementInterference,
};
pub(crate) use crate::screen_support::{FixedCredential, RecordingScreen};

pub(crate) struct NullScreen(pub(crate) TerminalGeometry);

impl ScreenModel for NullScreen {
    fn apply(&mut self, _bytes: &[u8]) {}
    fn resize(&mut self, geometry: TerminalGeometry) {
        self.0 = geometry;
    }
    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.0,
            rows: Vec::new(),
        }
    }
    fn repaint_bytes(&self) -> Vec<u8> {
        Vec::new()
    }
}

pub(crate) use crate::service_support::{
    credential_on, owned, service_as, service_on_version, service_on_version_with_settings,
    service_on_version_with_skills, service_over_with_context, service_with_context,
    service_with_credentials, service_with_environments_and_settings,
    service_with_environments_and_settings_service, service_with_environments_on_version,
    write_screen,
};

pub(crate) fn kernel() -> KernelId {
    KernelId::new("fake")
}

/// A service instance over the given store and adapter.
///
/// Calling this twice with the same store is the restart: the second instance
/// shares the durable record and nothing else.
pub(crate) fn service(store: Arc<FakeStore>, adapter: Arc<FakeAdapter>) -> Arc<SessionService> {
    service_over(store, adapter)
}

/// The same, over any store at all.
///
/// Taken as the trait rather than the double so a test can stand a badly
/// behaved implementation in front of the service — a store is a port, and how
/// the core survives one that misbehaves is worth being able to say.
pub(crate) fn service_over(
    store: Arc<dyn SessionStore>,
    adapter: Arc<FakeAdapter>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        crate::service::SessionPorts {
            context: None,
            credentials: None,
            skills: None,
            settings: None,
        },
        no_environments(),
        owned(),
    );
    // Retries are exercised here, not waited out: the production delay exists
    // for a provider under load, which no test is.
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A service instance over a particular set of environment profiles.
pub(crate) fn service_with_environments(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    profiles: Arc<EnvironmentProfileService>,
) -> Arc<SessionService> {
    service_over_with_environments(store, adapter, profiles)
}

/// The environment-aware fixture over an adversarial session store.
pub(crate) fn service_over_with_environments(
    store: Arc<dyn SessionStore>,
    adapter: Arc<FakeAdapter>,
    profiles: Arc<EnvironmentProfileService>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    SessionService::new(
        adapters,
        factory,
        store,
        crate::service::SessionPorts {
            context: None,
            credentials: None,
            skills: None,
            settings: None,
        },
        profiles,
        owned(),
    )
}

/// A profile service with nothing in it, for tests that do not care.
///
/// Every session resolves an environment, so one always has to be present;
/// this is the one that adds nothing to what the deployment mandates.
pub(crate) fn no_environments() -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(
        Arc::new(FakeProfileStore::default()),
        baseline(),
        None,
        None,
        crate::environment::AgentConfiguration {
            // The kernel the fake adapter is registered under. The default is
            // the one this deployment ships, and a fixture that left it there
            // would be asking for a runtime the test never registered.
            kernel: KernelId::new("fake"),
            ..Default::default()
        },
        unconfigured_settings(),
    )
}

/// A profile service whose deployment default pins these agent settings.
///
/// For tests about what a session is started with, now that the answer comes
/// from the deployment or a profile rather than from the request.
pub(crate) fn environments_defaulting_to(
    agent: crate::environment::AgentConfiguration,
) -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(
        Arc::new(FakeProfileStore::default()),
        baseline(),
        None,
        None,
        agent,
        unconfigured_settings(),
    )
}

pub(crate) fn user(subject: &str) -> Caller {
    Caller::new(
        SubjectId(subject.to_string()),
        Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
            Capability::ManageOwnEnvironmentProfiles,
        ]),
    )
}

pub(crate) fn admin() -> Caller {
    Caller::new(
        SubjectId("admin".into()),
        Authorization::from_capabilities([Capability::ReadAllSessions]),
    )
}

/// Wait until a session has finished starting, whichever way it went.
///
/// Creating a session now returns before its runtime exists, so a test that
/// asserts on a running runtime has to wait for one. Polls the store and the
/// in-process bring-up claim rather than sleeping a fixed time, so it is
/// neither flaky nor slower than the thing it waits for. A successful start
/// records `Ready` before publishing its live entry, and the claim spans both
/// boundaries.
pub(crate) async fn await_settled(
    svc: &Arc<SessionService>,
    caller: &Caller,
    id: &crate::ids::SessionId,
) -> crate::session::Session {
    for _ in 0..400 {
        if let Ok(s) = svc.get_session(caller, id).await {
            if s.status != crate::session::SessionStatus::Provisioning
                && !svc.starting.read().await.contains(id)
            {
                return s;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("session never finished starting");
}

pub(crate) fn spec() -> SessionSpec {
    SessionSpec {
        geometry: None,
        environment_profile: None,
        caller_credential: None,
        runtime_credential: None,
        requested_mode: None,
        quoted_rate_card: None,
        options: Default::default(),
    }
}
