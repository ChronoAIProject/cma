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

/// One attempt to reserve durable authority while a conversation link opens.
///
/// Opening is separate from publication: a replacement may wait on sign-in or
/// fail after reaching the runtime, and neither case may fence the active link
/// until the replacement is ready to install.
#[derive(Debug, Clone)]
pub struct ConversationOpeningLeaseRequest {
    /// Session whose runtime is being attached.
    pub session_id: SessionId,
    /// Exact process instance opening the link.
    pub holder: String,
    /// Never-reused identity for this opening attempt.
    pub token: String,
    /// Provider handle the resulting link must still name at publication.
    pub runtime_id: String,
    /// Runtime generation the resulting link must still name at publication.
    /// Zero is the compatibility generation and remains bound by `runtime_id`.
    pub runtime_generation: u64,
    /// Registered candidate reservation token for provisioning. Readoption of
    /// the already-authoritative runtime carries none.
    pub runtime_candidate_token: Option<String>,
    /// Requested lifetime, measured by the store's clock.
    pub ttl: Duration,
}

impl ConversationOpeningLeaseRequest {
    /// Build a request with a new ABA-resistant opening token.
    pub fn new(
        session_id: SessionId,
        holder: impl Into<String>,
        runtime_id: impl Into<String>,
        runtime_generation: u64,
        runtime_candidate: Option<&RuntimeCandidate>,
        ttl: Duration,
    ) -> Self {
        Self {
            session_id,
            holder: holder.into(),
            token: format!("conversation_opening_{}", uuid::Uuid::new_v4().simple()),
            runtime_id: runtime_id.into(),
            runtime_generation,
            runtime_candidate_token: runtime_candidate.map(|candidate| candidate.token.clone()),
            ttl,
        }
    }
}

/// Durable authority held while one conversation link is opening.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationOpeningLease {
    /// Session whose runtime is being attached.
    pub session_id: SessionId,
    /// Exact process instance holding this opening.
    pub holder: String,
    /// Never-reused identity of this opening attempt.
    pub token: String,
    /// Provider handle this opening must publish against.
    pub runtime_id: String,
    /// Exact durable runtime generation this opening must publish against.
    pub runtime_generation: u64,
    /// Registered candidate reservation token for provisioning, or none when
    /// readopting the already-authoritative runtime.
    pub runtime_candidate_token: Option<String>,
    /// Provider-operation lease token backing this opening, when coordination
    /// is enabled. Generation-zero compatibility openings carry none and are
    /// instead conditioned on the exact stored runtime handle.
    pub session_lease_token: Option<String>,
}

/// Durable identity of the one conversation link allowed to append inbound work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationLink {
    /// Session whose conversation this link carries.
    pub session_id: SessionId,
    /// Monotonic store-issued epoch. Zero is never published.
    pub epoch: u64,
    /// Opening token promoted into this link. Paired with the epoch so an
    /// unknown publication result can be replayed without advancing twice.
    pub token: String,
    /// Exact runtime generation this link was published for.
    pub runtime_generation: u64,
    /// Exact provider handle this link was published for.
    pub runtime_id: String,
}

/// Exact durable predicate attached to a conversation-originated commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationWriteAuthority {
    /// The sole early-write exception while the adapter is still opening.
    Opening {
        /// Exact pending opening claim.
        opening: ConversationOpeningLease,
        /// Current provider-operation lease, when coordination is enabled.
        session_lease: Option<SessionLease>,
    },
    /// An update or close from a successfully published link.
    Active(ConversationLink),
}

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
    /// Retire a runtime confirmed lost and provision its replacement.
    Recover,
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
    ///
    /// It may renew one still-active acquisition but must never become active
    /// again after that lease expires or is released.
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

/// One bounded request to fence input against a runtime handover.
///
/// Interaction leases are intentionally separate from provider-operation
/// leases. A provider operation needs a permanent idempotency receipt because
/// it may create or retire an external object. An interaction has no side
/// effect until after acquisition returns, and can occur for every keystroke;
/// retaining one receipt per request would make durable state grow without
/// bound. Stores instead issue a token from a monotonic per-session
/// epoch and retain only that epoch plus the active lease.
#[derive(Debug, Clone)]
pub struct SessionInteractionLeaseRequest {
    /// Session whose authoritative runtime will receive the interaction.
    pub session_id: SessionId,
    /// Process instance asking. Deployment identity alone is not unique.
    pub holder: String,
    /// Idempotency nonce for retrying an acquisition whose result was unknown.
    ///
    /// Unlike a provider-operation token, this is retained only while the
    /// interaction lease is active. The store returns its own monotonic token
    /// as part of [`SessionLease`]. A successful same-nonce retry must resolve
    /// every earlier overlapping invocation before its returned lease can be
    /// released; see [`crate::store::SessionStore::acquire_interaction_lease`].
    pub nonce: String,
    /// Exact authority and status the caller read before asking.
    pub boundary: SessionBoundary,
    /// Requested lifetime, measured by the store's clock.
    pub ttl: Duration,
}

impl SessionInteractionLeaseRequest {
    /// Build an interaction request against one exact durable boundary.
    pub fn new(
        session_id: SessionId,
        holder: impl Into<String>,
        boundary: SessionBoundary,
        ttl: Duration,
    ) -> Self {
        Self {
            session_id,
            holder: holder.into(),
            nonce: format!("interaction_request_{}", uuid::Uuid::new_v4().simple()),
            boundary,
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
    /// Acquisition token. A later acquisition always has a different one.
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

/// A provider runtime durably registered before it can become authoritative.
///
/// Once registration commits, reconciliation no longer has to guess whether
/// the provisioning caller survived: until an atomic authority switch consumes
/// this candidate, it remains cleanup debt. The provider-create/register gap is
/// separate. Recovery must match provider-visible `(session, generation)` back
/// to the durable reservation; registration alone cannot close a crash between
/// a successful provider create and this store write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCandidate {
    /// Session this candidate was provisioned for.
    pub session_id: SessionId,
    /// Reserved generation written into provider-visible metadata.
    pub generation: NonZeroU64,
    /// Opaque provider handle returned by provisioning.
    pub runtime_id: String,
    /// Reservation identity that makes registration and switching idempotent.
    pub token: String,
}

/// Why a provider runtime remains durably scheduled for exact cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeCleanupReason {
    /// Provisioning completed, but this candidate never became authority.
    UncommittedCandidate,
    /// An atomic switch replaced this formerly authoritative runtime.
    RetiredAuthority,
}

/// One exact provider runtime that still has to be reclaimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCleanup {
    /// Session whose operation created this debt.
    pub session_id: SessionId,
    /// Durable generation of the runtime. Zero identifies legacy authority and
    /// is deletable only by atomically comparing the same session ownership plus
    /// absent generation metadata after legacy writers are permanently fenced.
    pub generation: u64,
    /// Opaque provider handle. A handle alone is never sufficient authority to
    /// perform the cleanup.
    pub runtime_id: String,
    /// Stable debt identity used for exact acknowledgement and retry.
    pub token: String,
    /// Boundary that produced this debt.
    pub reason: RuntimeCleanupReason,
}

/// Result of atomically changing the session record and runtime authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAuthoritySwitch {
    /// Store revision after changing the record's runtime handle.
    pub revision: u64,
    /// Sequence of the runtime-incarnation marker committed with the switch.
    pub runtime_started_seq: u64,
    /// Exact boundary now durable and held by the caller's lease.
    pub boundary: SessionBoundary,
    /// Previous authority left for retryable cleanup, when one existed.
    pub cleanup: Option<RuntimeCleanup>,
}

/// Result of atomically retiring the currently authoritative runtime.
///
/// The generation is retained as the monotonic floor while the handle becomes
/// absent. The exact former authority becomes cleanup debt in the same commit,
/// so no crash can clear the only identity capable of reclaiming it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAuthorityRetirement {
    /// Store revision after clearing the record's runtime handle.
    pub revision: u64,
    /// Exact no-runtime boundary now durable and held by the caller's lease.
    pub boundary: SessionBoundary,
    /// Former current authority, retained for conditional provider cleanup.
    pub cleanup: RuntimeCleanup,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::event::{Event, EventKind, StoredEvent};
    use crate::ids::{KernelId, SubjectId};
    use crate::session::{RecoveryState, Session};
    use crate::store::{SessionStore, StoreError};
    use crate::test_support::{FakeAdapter, FakeStore};
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

    async fn interaction_request(
        store: &FakeStore,
        holder: &str,
        ttl: Duration,
    ) -> SessionInteractionLeaseRequest {
        let record = store.get(&record().session.id).await.unwrap().unwrap();
        let authority = store
            .runtime_authority(&record.session.id)
            .await
            .unwrap()
            .unwrap();
        SessionInteractionLeaseRequest::new(
            record.session.id.clone(),
            holder,
            SessionBoundary {
                status: record.session.status,
                authority,
            },
            ttl,
        )
    }

    #[tokio::test]
    async fn one_store_lease_excludes_another_service_instance() {
        let store = Arc::new(FakeStore::default());
        store.seed(&record()).await.unwrap();
        let first = interaction_request(&store, "replica-a", Duration::from_secs(30)).await;
        let second = request(
            &store,
            "replica-b",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;

        let held = store.acquire_interaction_lease(&first).await.unwrap();
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
    async fn an_expired_acquisition_token_can_never_become_active_again() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let expired = request(
            &store,
            "replica-a",
            SessionOperation::Readopt,
            Duration::from_millis(1),
        )
        .await;
        store.acquire_session_lease(&expired).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;

        assert!(matches!(
            store.acquire_session_lease(&expired).await,
            Err(StoreError::SessionLeaseLost { .. })
        ));
        let successor = request(
            &store,
            "replica-b",
            SessionOperation::Readopt,
            Duration::from_secs(30),
        )
        .await;
        assert_eq!(
            store
                .acquire_session_lease(&successor)
                .await
                .unwrap()
                .holder,
            "replica-b"
        );
    }

    #[tokio::test]
    async fn an_expired_holder_cannot_renew_or_release_its_aba_successor() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let expired = request(
            &store,
            "replica-a",
            SessionOperation::Lifecycle,
            Duration::ZERO,
        )
        .await;
        let old = store.acquire_session_lease(&expired).await.unwrap();
        let successor = request(
            &store,
            "replica-a",
            SessionOperation::Lifecycle,
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
    async fn bounded_interaction_acquire_and_release_are_idempotent_while_retained() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let request = interaction_request(&store, "replica-a", Duration::from_secs(30)).await;

        let first = store.acquire_interaction_lease(&request).await.unwrap();
        let replay = store.acquire_interaction_lease(&request).await.unwrap();
        assert_eq!(replay, first);
        store.release_session_lease(&first).await.unwrap();
        store
            .release_session_lease(&first)
            .await
            .expect("the bounded epoch replays an unknown release success");
        let successor = store.acquire_interaction_lease(&request).await.unwrap();
        assert_ne!(successor.token, first.token);
        store.release_session_lease(&successor).await.unwrap();
        store
            .release_session_lease(&first)
            .await
            .expect("an older release remains a no-op after its successor releases");
    }

    #[tokio::test(start_paused = true)]
    async fn expired_interaction_release_is_a_no_op_beside_a_provider_lease() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let interaction = interaction_request(&store, "replica-a", Duration::from_secs(1)).await;
        let expired = store.acquire_interaction_lease(&interaction).await.unwrap();

        tokio::time::advance(Duration::from_secs(2)).await;
        store
            .release_session_lease(&expired)
            .await
            .expect("releasing the expired current interaction succeeds");

        let provider_request = request(
            &store,
            "replica-b",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let provider = store
            .acquire_session_lease(&provider_request)
            .await
            .unwrap();
        store
            .release_session_lease(&expired)
            .await
            .expect("an old interaction release cannot disturb a provider lease");
        store
            .release_session_lease(&provider)
            .await
            .expect("the provider lease remains the exact active lease");
    }

    #[tokio::test]
    async fn forged_identity_at_the_current_interaction_epoch_cannot_release_it() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let request = interaction_request(&store, "replica-a", Duration::from_secs(30)).await;
        let lease = store.acquire_interaction_lease(&request).await.unwrap();
        let mut forged = lease.clone();
        forged.holder = "replica-b".into();

        assert!(matches!(
            store.release_session_lease(&forged).await,
            Err(StoreError::SessionLeaseLost { .. })
        ));
        store
            .release_session_lease(&lease)
            .await
            .expect("the exact current interaction identity remains active");
    }

    #[tokio::test]
    async fn malformed_interaction_release_token_is_reported_as_lease_loss() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let request = interaction_request(&store, "replica-a", Duration::from_secs(30)).await;
        let lease = store.acquire_interaction_lease(&request).await.unwrap();
        let mut malformed = lease.clone();
        malformed.token = "not-an-interaction-epoch".into();

        assert!(matches!(
            store.release_session_lease(&malformed).await,
            Err(StoreError::SessionLeaseLost { .. })
        ));
        store
            .release_session_lease(&lease)
            .await
            .expect("a malformed release cannot disturb the active interaction");
    }

    #[tokio::test]
    async fn an_interaction_nonce_is_validated_at_the_store_boundary() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let mut request = interaction_request(&store, "replica-a", Duration::from_secs(30)).await;
        request.nonce.clear();

        assert!(matches!(
            store.acquire_interaction_lease(&request).await,
            Err(StoreError::Malformed(message)) if message.contains("nonce")
        ));
    }

    #[tokio::test]
    async fn interaction_renewal_allows_status_drift_but_requires_runtime_authority() {
        let store = FakeStore::default();
        let mut changed = record();
        store.seed(&changed).await.unwrap();
        let request = interaction_request(&store, "replica-a", Duration::from_secs(30)).await;
        let lease = store.acquire_interaction_lease(&request).await.unwrap();

        changed.session.status = SessionStatus::Idle;
        changed.revision = store
            .commit_session_idempotent("interaction-status-drift", &changed, &[])
            .await
            .unwrap();
        store
            .renew_interaction_lease(&lease, Duration::from_secs(30))
            .await
            .expect("an interaction may retain work across its own status commit");
        assert!(matches!(
            store
                .renew_session_lease(&lease, Duration::from_secs(30))
                .await,
            Err(StoreError::Malformed(message)) if message.contains("interaction renewal")
        ));

        store
            .sessions
            .lock()
            .await
            .get_mut(&changed.session.id.0)
            .unwrap()
            .runtime_id = Some("runtime-successor".into());
        assert!(matches!(
            store
                .renew_interaction_lease(&lease, Duration::from_secs(30))
                .await,
            Err(StoreError::SessionLeaseLost { .. })
        ));
        store
            .release_session_lease(&lease)
            .await
            .expect("the exact interaction token clears after boundary drift");
    }

    #[tokio::test]
    async fn provider_renewal_still_requires_the_full_status_boundary() {
        let store = FakeStore::default();
        let mut changed = record();
        store.seed(&changed).await.unwrap();
        let request = request(
            &store,
            "replica-a",
            SessionOperation::Lifecycle,
            Duration::from_secs(30),
        )
        .await;
        let lease = store.acquire_session_lease(&request).await.unwrap();

        changed.session.status = SessionStatus::Idle;
        store
            .commit_session_idempotent("provider-status-drift", &changed, &[])
            .await
            .unwrap();
        assert!(matches!(
            store
                .renew_session_lease(&lease, Duration::from_secs(30))
                .await,
            Err(StoreError::SessionLeaseLost { .. })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn expired_interaction_nonce_gets_a_new_epoch_and_cannot_clear_its_successor() {
        let store = FakeStore::default();
        let mut seeded = record();
        seeded.skill_packs = crate::store::SkillPackState::Cleaned;
        store.seed(&seeded).await.unwrap();
        let interaction = interaction_request(&store, "replica-a", Duration::from_secs(1)).await;
        let old = store.acquire_interaction_lease(&interaction).await.unwrap();

        tokio::time::advance(Duration::from_secs(2)).await;
        let successor = store.acquire_interaction_lease(&interaction).await.unwrap();
        assert_ne!(old.token, successor.token);
        assert!(matches!(
            store.delete(&record().session.id).await,
            Err(StoreError::SessionLeaseHeld { .. })
        ));
        assert!(matches!(
            store
                .renew_interaction_lease(&old, Duration::from_secs(30))
                .await,
            Err(StoreError::SessionLeaseLost { .. })
        ));
        store
            .release_session_lease(&old)
            .await
            .expect("an older interaction release is a no-op beside its successor");

        let blocked = request(
            &store,
            "replica-b",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        assert!(matches!(
            store.acquire_session_lease(&blocked).await,
            Err(StoreError::SessionLeaseHeld { .. })
        ));

        tokio::time::advance(Duration::from_secs(2)).await;
        store
            .delete(&record().session.id)
            .await
            .expect("an expired interaction lease does not strand deletion");
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

    #[tokio::test]
    async fn registering_a_candidate_is_idempotent_but_cannot_change_its_handle() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let request = request(
            &store,
            "replica-a",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let lease = store.acquire_session_lease(&request).await.unwrap();
        let reservation = store
            .reserve_runtime_generation(&lease, "reservation-a")
            .await
            .unwrap();
        let candidate = store
            .register_runtime_candidate(&lease, &reservation, "runtime-candidate")
            .await
            .unwrap();

        assert_eq!(
            store
                .register_runtime_candidate(&lease, &reservation, "runtime-candidate")
                .await
                .unwrap(),
            candidate
        );
        assert!(matches!(
            store
                .register_runtime_candidate(&lease, &reservation, "runtime-different")
                .await,
            Err(StoreError::RuntimeCandidateChanged { .. })
        ));
        assert_eq!(
            store
                .runtime_cleanup_debts(&record().session.id)
                .await
                .unwrap(),
            [RuntimeCleanup {
                session_id: record().session.id,
                generation: 1,
                runtime_id: "runtime-candidate".into(),
                token: "candidate:reservation-a".into(),
                reason: RuntimeCleanupReason::UncommittedCandidate,
            }]
        );
    }

    #[tokio::test]
    async fn authority_switch_changes_handle_and_generation_as_one_idempotent_commit() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let request = request(
            &store,
            "replica-a",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let mut lease = store.acquire_session_lease(&request).await.unwrap();
        let reservation = store
            .reserve_runtime_generation(&lease, "reservation-a")
            .await
            .unwrap();
        let candidate = store
            .register_runtime_candidate(&lease, &reservation, "runtime-replacement")
            .await
            .unwrap();

        store.lose_runtime_switch_responses(1);
        assert!(matches!(
            store.switch_runtime_authority(&mut lease, &candidate).await,
            Err(StoreError::Unavailable(_))
        ));
        let switched = store
            .switch_runtime_authority(&mut lease, &candidate)
            .await
            .unwrap();
        assert_eq!(switched.revision, 1);
        assert_eq!(switched.runtime_started_seq, 1);
        assert_eq!(switched.boundary, lease.boundary);
        assert_eq!(switched.boundary.authority.generation, 1);
        assert_eq!(
            switched.boundary.authority.runtime_id.as_deref(),
            Some("runtime-replacement")
        );
        let durable = store.get(&record().session.id).await.unwrap().unwrap();
        assert_eq!(durable.runtime_id.as_deref(), Some("runtime-replacement"));
        assert_eq!(durable.session.last_seq, switched.runtime_started_seq);
        let events = store.events_since(&record().session.id, 0).await.unwrap();
        assert_eq!(events.len(), 1, "the retry must not duplicate the marker");
        let marker = events.into_iter().next().unwrap().into_known().unwrap();
        assert_eq!(marker.seq, switched.runtime_started_seq);
        assert!(!marker.appended_at.is_empty());
        assert!(matches!(
            marker.kind,
            EventKind::RuntimeStarted { generation } if generation == candidate.generation
        ));
        assert_eq!(
            store
                .runtime_authority(&record().session.id)
                .await
                .unwrap()
                .unwrap(),
            switched.boundary.authority
        );
        assert_eq!(
            store
                .switch_runtime_authority(&mut lease, &candidate)
                .await
                .unwrap(),
            switched,
            "an unknown commit result is retried without another switch"
        );
        assert_eq!(
            store
                .runtime_cleanup_debts(&record().session.id)
                .await
                .unwrap(),
            [RuntimeCleanup {
                session_id: record().session.id,
                generation: 0,
                runtime_id: "runtime-original".into(),
                token: "retired:reservation-a".into(),
                reason: RuntimeCleanupReason::RetiredAuthority,
            }]
        );
    }

    #[tokio::test]
    async fn authority_switch_receipt_replay_requires_the_exact_lease_identity() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let request = request(
            &store,
            "replica-a",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let mut lease = store.acquire_session_lease(&request).await.unwrap();
        let reservation = store
            .reserve_runtime_generation(&lease, "reservation-identity")
            .await
            .unwrap();
        let candidate = store
            .register_runtime_candidate(&lease, &reservation, "runtime-replacement")
            .await
            .unwrap();
        let switched = store
            .switch_runtime_authority(&mut lease, &candidate)
            .await
            .unwrap();

        let mut forged_holder = lease.clone();
        forged_holder.holder = "replica-forged".into();
        assert!(matches!(
            store
                .switch_runtime_authority(&mut forged_holder, &candidate)
                .await,
            Err(StoreError::RuntimeCandidateChanged { .. })
        ));

        let mut forged_operation = lease.clone();
        forged_operation.operation = SessionOperation::Recover;
        assert!(matches!(
            store
                .switch_runtime_authority(&mut forged_operation, &candidate)
                .await,
            Err(StoreError::RuntimeCandidateChanged { .. })
        ));

        assert_eq!(
            store
                .switch_runtime_authority(&mut lease, &candidate)
                .await
                .unwrap(),
            switched,
            "the exact completed lease still replays its receipt"
        );
        let events = store.events_since(&record().session.id, 0).await.unwrap();
        assert_eq!(events.len(), 1, "forged retries cannot append a marker");
    }

    #[tokio::test]
    async fn authority_switch_appends_after_the_exact_durable_tail() {
        let store = FakeStore::default();
        let mut seeded = record();
        seeded.session.last_seq = 1;
        store.seed(&seeded).await.unwrap();
        store.events.lock().await.extend([
            StoredEvent::from(Event {
                session_id: seeded.session.id.clone(),
                seq: 1,
                kind: EventKind::SessionCreated,
                appended_at: "2026-08-26T00:00:01Z".into(),
            }),
            StoredEvent::from(Event {
                session_id: seeded.session.id.clone(),
                seq: 2,
                kind: EventKind::TurnStarted,
                appended_at: "2026-08-26T00:00:02Z".into(),
            }),
        ]);
        let request = request(
            &store,
            "replica-a",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let mut lease = store.acquire_session_lease(&request).await.unwrap();
        let reservation = store
            .reserve_runtime_generation(&lease, "reservation-contiguous")
            .await
            .unwrap();
        let candidate = store
            .register_runtime_candidate(&lease, &reservation, "runtime-contiguous")
            .await
            .unwrap();

        let switched = store
            .switch_runtime_authority(&mut lease, &candidate)
            .await
            .unwrap();

        assert_eq!(switched.runtime_started_seq, 3);
        assert_eq!(
            store
                .get(&seeded.session.id)
                .await
                .unwrap()
                .unwrap()
                .session
                .last_seq,
            3,
            "the marker closes a physical tail that was ahead of the record"
        );
        let events = store.events_since(&seeded.session.id, 0).await.unwrap();
        assert_eq!(
            events.iter().map(|event| event.seq).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(matches!(
            events.last().map(|event| &event.kind),
            Some(crate::event::StoredEventKind::Known(
                EventKind::RuntimeStarted { generation }
            )) if *generation == candidate.generation
        ));
    }

    #[tokio::test]
    async fn authority_switch_failure_commits_neither_marker_nor_authority() {
        let store = FakeStore::default();
        let mut overflowing = record();
        overflowing.session.last_seq = u64::MAX;
        store.seed(&overflowing).await.unwrap();
        let request = request(
            &store,
            "replica-a",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let mut lease = store.acquire_session_lease(&request).await.unwrap();
        let original_lease = lease.clone();
        let reservation = store
            .reserve_runtime_generation(&lease, "reservation-overflow")
            .await
            .unwrap();
        let candidate = store
            .register_runtime_candidate(&lease, &reservation, "runtime-overflow")
            .await
            .unwrap();

        assert!(matches!(
            store
                .switch_runtime_authority(&mut lease, &candidate)
                .await,
            Err(StoreError::Malformed(detail)) if detail.contains("event sequence overflow")
        ));

        assert_eq!(lease, original_lease);
        let durable = store.get(&overflowing.session.id).await.unwrap().unwrap();
        assert_eq!(durable.revision, overflowing.revision);
        assert_eq!(durable.session.last_seq, u64::MAX);
        assert_eq!(durable.runtime_id, overflowing.runtime_id);
        assert_eq!(
            store
                .runtime_authority(&overflowing.session.id)
                .await
                .unwrap()
                .unwrap()
                .generation,
            0
        );
        assert!(store
            .events_since(&overflowing.session.id, 0)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .runtime_cleanup_debts(&overflowing.session.id)
                .await
                .unwrap()
                .iter()
                .filter(|cleanup| cleanup.reason == RuntimeCleanupReason::UncommittedCandidate)
                .count(),
            1,
            "the failed candidate remains cleanup debt rather than authority"
        );
    }

    #[tokio::test]
    async fn authority_retirement_atomically_clears_the_handle_and_creates_exact_debt() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let handover_request = request(
            &store,
            "replica-a",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let mut handover = store
            .acquire_session_lease(&handover_request)
            .await
            .unwrap();
        let reservation = store
            .reserve_runtime_generation(&handover, "reservation-current")
            .await
            .unwrap();
        let candidate = store
            .register_runtime_candidate(&handover, &reservation, "runtime-current")
            .await
            .unwrap();
        store
            .switch_runtime_authority(&mut handover, &candidate)
            .await
            .unwrap();
        store.release_session_lease(&handover).await.unwrap();

        let recovery_request = request(
            &store,
            "replica-b",
            SessionOperation::Recover,
            Duration::from_secs(30),
        )
        .await;
        let mut recovery = store
            .acquire_session_lease(&recovery_request)
            .await
            .unwrap();
        let retired = store
            .retire_runtime_authority(&mut recovery, "confirmed-loss")
            .await
            .unwrap();

        assert_eq!(retired.boundary, recovery.boundary);
        assert_eq!(retired.boundary.authority.generation, 1);
        assert!(retired.boundary.authority.runtime_id.is_none());
        assert_eq!(retired.cleanup.generation, 1);
        assert_eq!(retired.cleanup.runtime_id, "runtime-current");
        assert_eq!(
            retired.cleanup.reason,
            RuntimeCleanupReason::RetiredAuthority
        );
        assert!(store
            .get(&record().session.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_id
            .is_none());
        assert_eq!(
            store
                .retire_runtime_authority(&mut recovery, "confirmed-loss")
                .await
                .unwrap(),
            retired,
            "an unknown commit result returns the same retirement receipt"
        );
    }

    #[tokio::test]
    async fn authority_can_never_move_back_to_an_older_reserved_generation() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let request = request(
            &store,
            "replica-a",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let mut lease = store.acquire_session_lease(&request).await.unwrap();
        let older_reservation = store
            .reserve_runtime_generation(&lease, "reservation-older")
            .await
            .unwrap();
        let older = store
            .register_runtime_candidate(&lease, &older_reservation, "runtime-older")
            .await
            .unwrap();
        let newer_reservation = store
            .reserve_runtime_generation(&lease, "reservation-newer")
            .await
            .unwrap();
        let newer = store
            .register_runtime_candidate(&lease, &newer_reservation, "runtime-newer")
            .await
            .unwrap();

        store
            .switch_runtime_authority(&mut lease, &newer)
            .await
            .unwrap();
        assert!(matches!(
            store.switch_runtime_authority(&mut lease, &older).await,
            Err(StoreError::RuntimeCandidateChanged { .. })
        ));
        assert_eq!(
            store
                .runtime_authority(&record().session.id)
                .await
                .unwrap()
                .unwrap()
                .generation,
            newer.generation.get()
        );
        let markers = store.events_since(&record().session.id, 0).await.unwrap();
        assert_eq!(markers.len(), 1, "the stale candidate appends no marker");
        assert!(matches!(
            &markers[0].kind,
            crate::event::StoredEventKind::Known(EventKind::RuntimeStarted { generation })
                if *generation == newer.generation
        ));
    }

    #[tokio::test]
    async fn an_old_switch_receipt_cannot_regress_a_later_authority() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let request = request(
            &store,
            "replica-a",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let mut lease = store.acquire_session_lease(&request).await.unwrap();
        let stale_lease = lease.clone();
        let first_reservation = store
            .reserve_runtime_generation(&lease, "reservation-first")
            .await
            .unwrap();
        let first = store
            .register_runtime_candidate(&lease, &first_reservation, "runtime-first")
            .await
            .unwrap();
        store
            .switch_runtime_authority(&mut lease, &first)
            .await
            .unwrap();
        let second_reservation = store
            .reserve_runtime_generation(&lease, "reservation-second")
            .await
            .unwrap();
        let second = store
            .register_runtime_candidate(&lease, &second_reservation, "runtime-second")
            .await
            .unwrap();
        store
            .switch_runtime_authority(&mut lease, &second)
            .await
            .unwrap();

        let mut stale_retry = stale_lease;
        assert!(matches!(
            store
                .switch_runtime_authority(&mut stale_retry, &first)
                .await,
            Err(StoreError::RuntimeCandidateChanged { .. })
        ));
        assert_eq!(
            store
                .runtime_authority(&record().session.id)
                .await
                .unwrap()
                .unwrap()
                .generation,
            second.generation.get()
        );
    }

    #[tokio::test]
    async fn a_crash_before_switch_leaves_the_candidate_recoverable_not_authoritative() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let provision_request = request(
            &store,
            "replica-a",
            SessionOperation::Provision,
            Duration::from_millis(1),
        )
        .await;
        let mut expired = store
            .acquire_session_lease(&provision_request)
            .await
            .unwrap();
        let reservation = store
            .reserve_runtime_generation(&expired, "reservation-abandoned")
            .await
            .unwrap();
        let candidate = store
            .register_runtime_candidate(&expired, &reservation, "runtime-abandoned")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;

        let reclaim_request = request(
            &store,
            "replica-b",
            SessionOperation::Reclaim,
            Duration::from_secs(30),
        )
        .await;
        let reclaim = store.acquire_session_lease(&reclaim_request).await.unwrap();
        let cleanup = store
            .runtime_cleanup_debts(&record().session.id)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(cleanup.runtime_id, candidate.runtime_id);
        assert_eq!(cleanup.generation, candidate.generation.get());
        assert_eq!(cleanup.reason, RuntimeCleanupReason::UncommittedCandidate);
        store
            .acknowledge_runtime_cleanup(&reclaim, &cleanup)
            .await
            .unwrap();

        assert!(store
            .runtime_cleanup_debts(&record().session.id)
            .await
            .unwrap()
            .is_empty());
        assert!(matches!(
            store
                .switch_runtime_authority(&mut expired, &candidate)
                .await,
            Err(StoreError::SessionLeaseLost { .. })
        ));
        assert_eq!(
            store
                .runtime_authority(&record().session.id)
                .await
                .unwrap()
                .unwrap()
                .runtime_id
                .as_deref(),
            Some("runtime-original")
        );
    }

    #[tokio::test]
    async fn cleanup_acknowledgement_cannot_remove_current_authority() {
        let store = FakeStore::default();
        store.seed(&record()).await.unwrap();
        let handover_request = request(
            &store,
            "replica-a",
            SessionOperation::Handover,
            Duration::from_secs(30),
        )
        .await;
        let mut handover = store
            .acquire_session_lease(&handover_request)
            .await
            .unwrap();
        let reservation = store
            .reserve_runtime_generation(&handover, "reservation-a")
            .await
            .unwrap();
        let candidate = store
            .register_runtime_candidate(&handover, &reservation, "runtime-current")
            .await
            .unwrap();
        store
            .switch_runtime_authority(&mut handover, &candidate)
            .await
            .unwrap();
        store.release_session_lease(&handover).await.unwrap();

        let reclaim_request = request(
            &store,
            "replica-b",
            SessionOperation::Reclaim,
            Duration::from_secs(30),
        )
        .await;
        let reclaim = store.acquire_session_lease(&reclaim_request).await.unwrap();
        let retired = store
            .runtime_cleanup_debts(&record().session.id)
            .await
            .unwrap()
            .pop()
            .unwrap();
        store
            .acknowledge_runtime_cleanup(&reclaim, &retired)
            .await
            .unwrap();
        store
            .acknowledge_runtime_cleanup(&reclaim, &retired)
            .await
            .unwrap();

        let invented = RuntimeCleanup {
            session_id: record().session.id,
            generation: candidate.generation.get(),
            runtime_id: candidate.runtime_id,
            token: "invented-current-cleanup".into(),
            reason: RuntimeCleanupReason::RetiredAuthority,
        };
        assert!(matches!(
            store.acknowledge_runtime_cleanup(&reclaim, &invented).await,
            Err(StoreError::RuntimeCleanupChanged { .. })
        ));
        assert_eq!(
            store
                .runtime_authority(&record().session.id)
                .await
                .unwrap()
                .unwrap()
                .runtime_id
                .as_deref(),
            Some("runtime-current")
        );
        store.release_session_lease(&reclaim).await.unwrap();
        assert!(matches!(
            store.delete(&record().session.id).await,
            Err(StoreError::SessionCleanupPending { .. })
        ));
        assert!(store.get(&record().session.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn conditional_teardown_compares_session_generation_and_legacy_absence() {
        use layer3_adapter::{KernelAdapter, RuntimeHandle, RuntimeIdentity};

        let adapter = FakeAdapter::default();
        adapter
            .running
            .lock()
            .await
            .insert("runtime-reused".into(), Some("sesn_other".into()));
        adapter.generations.lock().await.insert(
            "runtime-reused".into(),
            NonZeroU64::new(2).expect("non-zero fixture"),
        );

        let stale = RuntimeIdentity {
            handle: RuntimeHandle("runtime-reused".into()),
            session: "sesn_other".into(),
            generation: NonZeroU64::new(1),
        };
        assert!(matches!(
            adapter.terminate_if_identity(&stale).await,
            Err(layer3_adapter::AdapterError::Rejected(_))
        ));
        assert!(adapter.running.lock().await.contains_key("runtime-reused"));
        assert!(adapter.terminated.lock().await.is_empty());

        let wrong_session = RuntimeIdentity {
            handle: RuntimeHandle("runtime-reused".into()),
            session: "sesn_reused_elsewhere".into(),
            generation: NonZeroU64::new(2),
        };
        assert!(matches!(
            adapter.terminate_if_identity(&wrong_session).await,
            Err(layer3_adapter::AdapterError::Rejected(_))
        ));
        assert!(
            adapter.running.lock().await.contains_key("runtime-reused"),
            "a per-session generation match is not global identity proof"
        );

        let current = RuntimeIdentity {
            handle: RuntimeHandle("runtime-reused".into()),
            session: "sesn_other".into(),
            generation: NonZeroU64::new(2),
        };
        adapter.terminate_if_identity(&current).await.unwrap();
        assert!(!adapter.running.lock().await.contains_key("runtime-reused"));

        adapter
            .running
            .lock()
            .await
            .insert("runtime-legacy".into(), Some("sesn_legacy".into()));
        let recorded = RuntimeIdentity {
            handle: RuntimeHandle("runtime-legacy".into()),
            session: "sesn_legacy".into(),
            generation: NonZeroU64::new(3),
        };
        assert!(matches!(
            adapter.terminate_if_identity(&recorded).await,
            Err(layer3_adapter::AdapterError::Rejected(_))
        ));
        assert!(adapter.running.lock().await.contains_key("runtime-legacy"));

        let legacy = RuntimeIdentity {
            handle: RuntimeHandle("runtime-legacy".into()),
            session: "sesn_legacy".into(),
            generation: None,
        };
        adapter.terminate_if_identity(&legacy).await.unwrap();
        assert!(!adapter.running.lock().await.contains_key("runtime-legacy"));
    }
}
