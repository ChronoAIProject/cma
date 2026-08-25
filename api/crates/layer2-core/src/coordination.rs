//! Durable authority over external operations on one session runtime.
//!
//! A control-plane process is not authority: replicas overlap, requests are
//! cancelled, and a process can disappear while a provider call is in flight.
//! These values therefore cross the store boundary. Implementations use their
//! own authoritative clock for expiry; callers state only a duration.

use std::num::NonZeroU64;
use std::time::Duration;

use crate::ids::SessionId;
use crate::session::SessionStatus;
use crate::store::SessionRecord;

/// The provider runtime currently authorised to serve a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAuthority {
    /// Monotonic generation within the session. Zero identifies legacy state.
    pub generation: u64,
    /// Opaque provider handle, when the session currently names a runtime.
    pub runtime_id: Option<String>,
}

/// Exact session state against which an external operation may begin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBoundary {
    /// Lifecycle state observed by the caller.
    pub status: SessionStatus,
    /// Runtime authority observed by the caller.
    pub authority: RuntimeAuthority,
}

impl SessionBoundary {
    /// Pair one record read with its separately stored runtime generation.
    pub fn for_record(record: &SessionRecord, generation: u64) -> Self {
        Self {
            status: record.session.status,
            authority: RuntimeAuthority {
                generation,
                runtime_id: record.runtime_id.clone(),
            },
        }
    }
}

/// Why a replica needs exclusive authority over the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOperation {
    /// Deliver input or another mutation to the authoritative runtime.
    Interact,
    /// Provision a runtime for a session that has none.
    Provision,
    /// Capture and replace an authoritative runtime.
    Handover,
    /// Attach this replica to an already authoritative runtime.
    Readopt,
    /// Reclaim a runtime only after authority has been rechecked.
    Reclaim,
    /// Stop, terminate, or otherwise change desired lifecycle state.
    Lifecycle,
}

/// One attempt to acquire durable exclusive authority.
#[derive(Debug, Clone)]
pub struct SessionLeaseRequest {
    /// Session being operated on.
    pub session_id: SessionId,
    /// Process instance asking. Deployment identity alone is not unique.
    pub holder: String,
    /// Idempotency and ABA token generated once per acquisition attempt.
    pub token: String,
    /// Exact authority and status the caller read before asking.
    pub boundary: SessionBoundary,
    /// Operation being fenced, for diagnostics and exact retry matching.
    pub operation: SessionOperation,
    /// Requested lifetime, measured by the store's clock.
    pub ttl: Duration,
}

impl SessionLeaseRequest {
    /// Build a request with a new, unguessable acquisition token.
    pub fn new(
        session_id: SessionId,
        holder: impl Into<String>,
        boundary: SessionBoundary,
        operation: SessionOperation,
        ttl: Duration,
    ) -> Self {
        Self {
            session_id,
            holder: holder.into(),
            token: format!("slease_{}", uuid::Uuid::new_v4().simple()),
            boundary,
            operation,
            ttl,
        }
    }
}

/// A durable exclusive lease returned by the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLease {
    /// Session this lease fences.
    pub session_id: SessionId,
    /// Exact process instance holding it.
    pub holder: String,
    /// Acquisition token. A later holder always has a different one.
    pub token: String,
    /// Boundary whose external side effects this lease authorises.
    pub boundary: SessionBoundary,
    /// Operation named by the acquisition.
    pub operation: SessionOperation,
}

/// A never-reused generation reserved before provisioning begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReservation {
    /// Session receiving the candidate runtime.
    pub session_id: SessionId,
    /// Generation to write into provider-visible metadata.
    pub generation: NonZeroU64,
    /// Exact reservation identity for idempotent retries and cleanup.
    pub token: String,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::ids::{KernelId, SubjectId};
    use crate::session::{RecoveryState, Session};
    use crate::store::{SessionStore, StoreError};
    use crate::test_support::FakeStore;
    use crate::{AgentOptions, TerminalGeometry};

    fn record() -> crate::store::SessionRecord {
        let mut record = crate::store::SessionRecord::new(Session {
            id: SessionId("sesn_coordination".into()),
            owner: SubjectId("subject-coordination".into()),
            kernel: KernelId::new("fake"),
            status: SessionStatus::Ready,
            recovery_state: RecoveryState::Normal,
            stop_reason: None,
            options: AgentOptions::default(),
            environment_profile: None,
            geometry: TerminalGeometry::default(),
            last_seq: 0,
            runtime_version: None,
            rotations: 0,
            rotated_at: None,
            capabilities: Default::default(),
            granted_services: None,
            granted_skills: None,
            pricing: None,
            created_at: "2026-08-26T00:00:00Z".into(),
        });
        record.runtime_id = Some("runtime-original".into());
        record
    }

    async fn request(
        store: &FakeStore,
        holder: &str,
        operation: SessionOperation,
        ttl: Duration,
    ) -> SessionLeaseRequest {
        let record = store.get(&record().session.id).await.unwrap().unwrap();
        let authority = store
            .runtime_authority(&record.session.id)
            .await
            .unwrap()
            .unwrap();
        SessionLeaseRequest::new(
            record.session.id.clone(),
            holder,
            SessionBoundary {
                status: record.session.status,
                authority,
            },
            operation,
            ttl,
        )
    }

    #[tokio::test]
    async fn one_store_lease_excludes_another_service_instance() {
        let store = Arc::new(FakeStore::default());
        store.seed(&record()).await.unwrap();
        let first = request(
            &store,
            "replica-a",
            SessionOperation::Interact,
            Duration::from_secs(30),
        )
        .await;
        let second = request(
            &store,
            "replica-b",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;

        let held = store.acquire_session_lease(&first).await.unwrap();
        assert!(matches!(
            store.acquire_session_lease(&second).await,
            Err(StoreError::SessionLeaseHeld { id }) if id == record().session.id
        ));
        store.release_session_lease(&held).await.unwrap();
        assert_eq!(
            store.acquire_session_lease(&second).await.unwrap().holder,
            "replica-b"
        );
    }

    #[tokio::test]
    async fn retrying_one_acquisition_token_is_idempotent() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let request = request(
            &store,
            "replica-a",
            SessionOperation::Readopt,
            Duration::from_secs(30),
        )
        .await;
        let first = store.acquire_session_lease(&request).await.unwrap();
        let retry = store.acquire_session_lease(&request).await.unwrap();
        assert_eq!(retry, first);
    }

    #[tokio::test]
    async fn an_expired_holder_cannot_renew_or_release_its_aba_successor() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let expired = request(
            &store,
            "replica-a",
            SessionOperation::Interact,
            Duration::ZERO,
        )
        .await;
        let old = store.acquire_session_lease(&expired).await.unwrap();
        let successor = request(
            &store,
            "replica-a",
            SessionOperation::Interact,
            Duration::from_secs(30),
        )
        .await;
        let current = store.acquire_session_lease(&successor).await.unwrap();

        assert_ne!(old.token, current.token);
        assert!(matches!(
            store
                .renew_session_lease(&old, Duration::from_secs(30))
                .await,
            Err(StoreError::SessionLeaseLost { .. })
        ));
        assert!(matches!(
            store.release_session_lease(&old).await,
            Err(StoreError::SessionLeaseLost { .. })
        ));
        assert!(matches!(
            store
                .acquire_session_lease(
                    &request(
                        &store,
                        "replica-b",
                        SessionOperation::Reclaim,
                        Duration::from_secs(30),
                    )
                    .await
                )
                .await,
            Err(StoreError::SessionLeaseHeld { .. })
        ));
    }

    #[tokio::test]
    async fn acquisition_requires_the_exact_status_and_runtime_authority() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let mut stale = request(
            &store,
            "replica-a",
            SessionOperation::Lifecycle,
            Duration::from_secs(30),
        )
        .await;
        stale.boundary.status = SessionStatus::Idle;
        assert!(matches!(
            store.acquire_session_lease(&stale).await,
            Err(StoreError::SessionAuthorityChanged { .. })
        ));

        stale.boundary.status = SessionStatus::Ready;
        stale.boundary.authority.runtime_id = Some("runtime-stale".into());
        assert!(matches!(
            store.acquire_session_lease(&stale).await,
            Err(StoreError::SessionAuthorityChanged { .. })
        ));
    }

    #[tokio::test]
    async fn a_holder_can_advance_only_to_the_status_it_just_committed() {
        let store = FakeStore::default();
        let mut changed = record();
        store.seed(&changed).await.unwrap();
        let request = request(
            &store,
            "replica-a",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let mut lease = store.acquire_session_lease(&request).await.unwrap();

        changed.session.status = SessionStatus::Rotating;
        changed.revision = store
            .commit_session_idempotent("mark-rotating", &changed, &[])
            .await
            .unwrap();
        let advanced = SessionBoundary::for_record(&changed, lease.boundary.authority.generation);
        store
            .advance_session_lease(&mut lease, &advanced)
            .await
            .unwrap();
        store
            .renew_session_lease(&lease, Duration::from_secs(30))
            .await
            .unwrap();

        let mut invented = advanced;
        invented.status = SessionStatus::Idle;
        assert!(matches!(
            store.advance_session_lease(&mut lease, &invented).await,
            Err(StoreError::SessionLeaseLost { .. })
        ));
    }

    #[tokio::test]
    async fn runtime_generations_are_idempotent_and_never_reused() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let first_request = request(
            &store,
            "replica-a",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let first_lease = store.acquire_session_lease(&first_request).await.unwrap();
        let first = store
            .reserve_runtime_generation(&first_lease, "reservation-a")
            .await
            .unwrap();
        assert_eq!(
            store
                .reserve_runtime_generation(&first_lease, "reservation-a")
                .await
                .unwrap(),
            first
        );
        store.release_session_lease(&first_lease).await.unwrap();

        let second_request = request(
            &store,
            "replica-b",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let second_lease = store.acquire_session_lease(&second_request).await.unwrap();
        assert!(matches!(
            store
                .reserve_runtime_generation(&second_lease, "reservation-a")
                .await,
            Err(StoreError::RuntimeReservationChanged { .. })
        ));
        let second = store
            .reserve_runtime_generation(&second_lease, "reservation-b")
            .await
            .unwrap();
        assert_eq!(first.generation.get(), 1);
        assert_eq!(second.generation.get(), 2);
    }
}
