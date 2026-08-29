//! Persistence, provider, billing, pricing, and clock boundaries.

use std::fmt;

use async_trait::async_trait;

use super::super::LaunchId;
use super::model::*;
use crate::{Page, PageRequest, SessionId, SubjectId};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SponsorshipStoreError {
    #[error("sponsorship persistence is unavailable: {0}")]
    Unavailable(String),
    #[error("stored sponsorship data is unusable: {0}")]
    Malformed(String),
    #[error("the sponsorship resource already exists with different contents")]
    Conflict,
    #[error("the sponsorship generation changed")]
    StaleGeneration,
    #[error("the durable sponsorship kill-switch epoch changed")]
    GateChanged,
    #[error("the durable Launch authority changed")]
    LaunchAuthorityChanged,
    #[error("the accepted sponsored terms changed")]
    AcceptanceChanged,
    #[error("the sponsored grant is expired or unavailable")]
    GrantUnavailable,
    #[error("the per-Launch sponsored quota is exhausted")]
    LaunchQuotaExceeded,
    #[error("the aggregate sponsored quota is exhausted")]
    AggregateQuotaExceeded,
    #[error("the per-Launch sponsored budget is exhausted")]
    LaunchBudgetExceeded,
    #[error("the aggregate sponsored budget is exhausted")]
    AggregateBudgetExceeded,
    #[error("the sponsored request rate is exhausted")]
    RateExceeded,
    #[error("the sponsored Launch concurrency limit is exhausted")]
    ConcurrentLaunchLimit,
}

/// An atomic credential-record/material read from encrypted persistence.
///
/// Active and pending records carry a decrypted, zeroizing secret only for
/// the duration of the provider operation. A definitively revoked record must
/// return no secret because persistence has already erased current and staged
/// ciphertext.
pub struct StoredSponsoredCredential {
    pub record: SponsoredCredentialRecord,
    pub secret: Option<ProviderCredentialSecret>,
}

impl StoredSponsoredCredential {
    pub fn validate(&self) -> Result<(), SponsorshipStoreError> {
        self.record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        let material_is_retained =
            self.record.material_state == SponsoredCredentialMaterialState::Retained;
        if material_is_retained != self.secret.is_some() {
            return Err(SponsorshipStoreError::Malformed(
                "credential material does not match its durable retention state".into(),
            ));
        }
        Ok(())
    }

    pub fn into_required_secret(
        self,
    ) -> Result<(SponsoredCredentialRecord, ProviderCredentialSecret), SponsorshipStoreError> {
        self.validate()?;
        let secret = self.secret.ok_or_else(|| {
            SponsorshipStoreError::Malformed(
                "retired credential material cannot authorize a provider operation".into(),
            )
        })?;
        Ok((self.record, secret))
    }
}

/// One policy and its accounting read from the same durable document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSponsorshipPolicy {
    pub policy: SponsorshipPolicy,
    pub usage: SponsorshipPolicyUsage,
}

impl StoredSponsorshipPolicy {
    pub fn validate(&self) -> Result<(), SponsorshipStoreError> {
        self.policy
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        let exposure = self
            .usage
            .exposure()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if !exposure.quota.fits_within(self.policy.aggregate_quota)
            || exposure.spend_micros > self.policy.aggregate_budget_micros
            || self.usage.active_launches > self.policy.max_concurrent_launches
        {
            return Err(SponsorshipStoreError::Malformed(
                "policy accounting exceeds its durable authority".into(),
            ));
        }
        Ok(())
    }
}

/// A bounded, opaque continuation used only by internal recovery workers.
///
/// Unlike owner-facing pages, recovery rows are mutable and are ordered by the
/// durable update time at which they became work. The store owns the cursor
/// encoding so callers cannot couple themselves to collection fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorshipRecoveryPageRequest {
    limit: u32,
    cursor: Option<String>,
}

impl SponsorshipRecoveryPageRequest {
    pub fn new(limit: u32, cursor: Option<String>) -> Self {
        Self {
            limit: limit.clamp(1, MAX_SPONSORED_AUDIT_PAGE as u32),
            cursor,
        }
    }

    pub fn limit(&self) -> u32 {
        self.limit
    }

    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }
}

impl Default for SponsorshipRecoveryPageRequest {
    fn default() -> Self {
        Self::new(50, None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorshipRecoveryPage<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SponsorshipCiphertextRecordKind {
    Credential,
    GrantActivation,
    Grant,
}

/// One exact old-key record selected for compare-and-swap re-encryption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorshipCiphertextRewrapCandidate {
    pub kind: SponsorshipCiphertextRecordKind,
    pub record_id: String,
    pub retiring_key_id: String,
    /// Digest of the exact raw durable document selected by discovery. Stores
    /// must treat a mismatch as lost compare-and-swap work, never as authority
    /// to re-encrypt a newer domain revision under this stale candidate.
    pub document_fingerprint: String,
}

/// Versioned durable identity for one credential-registration request and its
/// complete secret-bearing command. Request aliases locate a prior binding;
/// command aliases distinguish an exact retry from changed owner, provider,
/// policy, or credential material without persisting a plaintext oracle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsoredCredentialRegistrationIdentity {
    pub request: SponsorshipOperationalPseudonymSet,
    pub command: SponsorshipOperationalPseudonymSet,
}

/// Atomic durable operations required by sponsorship enforcement.
///
/// Reservation and reconciliation implementations must update the policy,
/// grant, stable request record, and audit event in one transaction. A caller
/// retrying the same stable request and caller-input idempotency fingerprint
/// receives the prior result even if database time or current pricing has
/// changed; a changed idempotency fingerprint is a conflict.
#[async_trait]
pub trait SponsorshipStore: Send + Sync {
    async fn gate_fence(&self) -> Result<SponsorshipGateFence, SponsorshipStoreError>;

    /// Atomically increments the durable epoch, engages the kill switch, records
    /// a durable engagement identity, and appends the caller audit before any
    /// drain work. Never-attempted `AwaitingBilling` and `AwaitingProvider`
    /// intents are drained in bounded follow-up transactions: each becomes a
    /// terminal `Denied` intent with `AuthorityInvalidated`, releases its held
    /// capacity exactly once, and writes deterministic per-activation evidence
    /// atomically. An exact engagement retry resumes the drain without advancing
    /// the epoch. Attempting or ambiguous external operations retain capacity
    /// for read-back and cleanup.
    async fn engage_kill_switch(
        &self,
        expected_epoch: u64,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsorshipGateFence, SponsorshipStoreError>;

    async fn launch_authority(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredLaunchAuthority>, SponsorshipStoreError>;

    /// Resolve a durable registration before contacting the provider. An exact
    /// request/command replay returns the stored record and may only add current
    /// pseudonym aliases. A located request with no matching command candidate
    /// is a conflict. Neither outcome may append audit evidence or mutate the
    /// credential.
    async fn credential_registration(
        &self,
        identity: &SponsoredCredentialRegistrationIdentity,
    ) -> Result<Option<SponsoredCredentialRecord>, SponsorshipStoreError>;

    /// Atomically bind the versioned request and command aliases to the
    /// credential and enforce one record per provider authority. The
    /// same request and stable authority/command bindings return the stored
    /// record without another audit append. A changed command is a conflict,
    /// including when the deterministic record ID is unchanged. Implementations
    /// must repeat the request/command binding lookup inside the insertion
    /// transaction so concurrent registrations cannot bypass early replay.
    async fn insert_credential(
        &self,
        identity: &SponsoredCredentialRegistrationIdentity,
        provider_identity: &SponsorshipOperationalPseudonymSet,
        record: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError>;

    /// Read credential authority and lifecycle metadata without loading or
    /// decrypting current or staged provider credential material.
    async fn credential_record(
        &self,
        id: &SponsoredCredentialId,
    ) -> Result<Option<SponsoredCredentialRecord>, SponsorshipStoreError>;

    /// List only credentials controlled by `owner`, newest first by immutable
    /// `(created_at, id)`. Mutable updates must not move rows between pages.
    async fn credentials_for_owner(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<SponsoredCredentialRecord>, SponsorshipStoreError>;

    /// Atomically read the record and its matching current secret. Definitive
    /// revocation returns a validated record with `secret=None`.
    async fn credential(
        &self,
        id: &SponsoredCredentialId,
    ) -> Result<Option<StoredSponsoredCredential>, SponsorshipStoreError>;

    /// Read a terminal credential mutation by its durable caller binding. This
    /// ledger is independent of the mutable credential head and is retained for
    /// the full supported replay lifetime. Completion/abort methods below must
    /// update it atomically with credential and encrypted-material state.
    async fn completed_credential_operation(
        &self,
        id: &SponsoredCredentialId,
        kind: ProviderOperationKind,
        client_request_identity: &SponsorshipOperationalPseudonymSet,
    ) -> Result<Option<CompletedProviderOperation>, SponsorshipStoreError>;

    /// Replace a credential record and optionally its current encrypted secret
    /// without changing its material-retention state. This path must never
    /// retire material; definitive retirement is exclusive to
    /// `complete_credential_revocation`.
    async fn replace_credential(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        secret: Option<&ProviderCredentialSecret>,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError>;

    async fn stage_credential_rotation(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        replacement: &ProviderCredentialSecret,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError>;

    async fn pending_credential_rotation_secret(
        &self,
        id: &SponsoredCredentialId,
    ) -> Result<Option<ProviderCredentialSecret>, SponsorshipStoreError>;

    /// Promote the encrypted pending replacement into the current slot in the
    /// same write that clears `RotationPending`. The transaction must also
    /// enforce uniqueness of `(provider, provider_credential_id_digest)`
    /// against every other durable credential. If that uniqueness CAS loses
    /// after the provider rotated, both predecessor and replacement material
    /// remain retained under the pending record for ambiguous recovery; no
    /// duplicate active authority may be published.
    async fn complete_credential_rotation(
        &self,
        expected_generation: u64,
        provider_identity: &SponsorshipOperationalPseudonymSet,
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError>;

    /// Discard the encrypted pending replacement after a definitive provider
    /// rejection while retaining the predecessor as the current secret.
    async fn abort_credential_rotation(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError>;

    /// Commit a definitive provider revocation receipt, mark material retired,
    /// and erase both current and staged encrypted secrets in the same write.
    /// Ambiguous, unavailable, or rejected operations must never call this.
    async fn complete_credential_revocation(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError>;

    async fn insert_policy(
        &self,
        policy: &SponsorshipPolicy,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsorshipPolicy, SponsorshipStoreError>;

    async fn policy(
        &self,
        id: &SponsorshipPolicyId,
    ) -> Result<Option<SponsorshipPolicy>, SponsorshipStoreError>;

    /// List owner-controlled policies and their co-located accounting, newest
    /// first by immutable `(created_at, id)`.
    async fn policies_for_owner(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<StoredSponsorshipPolicy>, SponsorshipStoreError>;

    async fn policy_usage(
        &self,
        id: &SponsorshipPolicyId,
    ) -> Result<SponsorshipPolicyUsage, SponsorshipStoreError>;

    async fn replace_policy(
        &self,
        expected_generation: u64,
        policy: &SponsorshipPolicy,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsorshipPolicy, SponsorshipStoreError>;

    /// Insert an acceptance after atomically fencing Launch and gate authority.
    /// Before those mutable checks, an existing acceptance with the same
    /// idempotency hash must be compared to the stable candidate while ignoring
    /// only service-generated `accepted_at` and `digest`. An exact concurrent
    /// replay returns the stored winner; a changed payload is a conflict.
    async fn insert_acceptance(
        &self,
        command: PersistSponsoredAcceptance,
    ) -> Result<SponsoredGrantAcceptance, SponsorshipStoreError>;

    async fn acceptance_for_launch(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredGrantAcceptance>, SponsorshipStoreError>;

    /// Resolve an acceptance for an idempotent caller replay. If its frozen
    /// canonical request identity matches a configured candidate, persistence
    /// replaces the private alias set with the complete bounded candidates.
    async fn acceptance_for_request(
        &self,
        launch_id: &LaunchId,
        idempotency_candidates: &SponsorshipOperationalPseudonymSet,
    ) -> Result<Option<SponsoredGrantAcceptance>, SponsorshipStoreError>;

    /// Insert the Launch-scoped intent before the billing observation or any
    /// provider write, or take over its expired lease. Acquiring a claim also
    /// reserves the full aggregate allocation and one concurrency slot before
    /// billing or provider observation. An unexpired different holder receives
    /// `acquired=false` and must not contact billing or the provider. The store
    /// must reject a candidate carrying a lease and create/compare lease times
    /// from database time in the same CAS (for MongoDB, `$$NOW` or a
    /// transactionally equivalent server-time expression), never caller time.
    async fn claim_grant_activation(
        &self,
        command: ClaimGrantActivation,
    ) -> Result<GrantActivationClaim, SponsorshipStoreError>;

    async fn grant_activation(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<GrantActivationIntent>, SponsorshipStoreError>;

    /// Claim remediation for provider authority that was issued but could not
    /// pass the final Launch/gate/policy/credential CAS. This deliberately does
    /// not require those now-invalid dependencies. The store must create or
    /// compare the lease using authoritative database time in the cleanup CAS.
    async fn claim_grant_activation_cleanup(
        &self,
        command: ClaimGrantActivationCleanup,
    ) -> Result<GrantActivationClaim, SponsorshipStoreError>;

    /// Acquire recovery of an in-flight billing/provider operation only after
    /// the store atomically proves its original gate/Launch/acceptance/policy/
    /// credential dependencies are no longer current. Invalidation and lease
    /// takeover must use the same database-time transaction/CAS boundary.
    async fn claim_invalidated_grant_activation(
        &self,
        command: ClaimInvalidatedGrantActivation,
    ) -> Result<GrantActivationClaim, SponsorshipStoreError>;

    /// After a confirmed billing observation and before provider dispatch,
    /// atomically revalidate the original gate, Launch, acceptance, policy,
    /// credential, material, and capacity authority using database time. A
    /// valid intent transitions directly to `ProviderAttempting`. An invalid
    /// intent transitions to terminal `Denied(AuthorityInvalidated)` and
    /// releases its capacity exactly once. Callers may issue a provider grant
    /// only when the returned state is `ProviderAttempting`.
    async fn begin_provider_grant_issue(
        &self,
        command: BeginProviderGrantIssue,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError>;

    async fn update_grant_activation(
        &self,
        expected_revision: u64,
        holder: &str,
        intent: &GrantActivationIntent,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError>;

    /// Atomically insert the grant, advance the activation to `Committed`, and
    /// append its audit event. This transfers ownership of the already-counted
    /// allocation/concurrency hold from the intent to the grant; it MUST NOT
    /// increment policy allocation or concurrency again. Policy and receipt
    /// expiry are checked with authoritative database time in this transaction,
    /// never `CommitGrantActivation.now`.
    async fn commit_grant_activation(
        &self,
        command: CommitGrantActivation,
    ) -> Result<SponsoredGrant, SponsorshipStoreError>;

    /// Atomically record confirmed provider cleanup and release the activation
    /// allocation/concurrency hold exactly once. Every definitive denial,
    /// pre-dispatch deferral, invalidation, and cleanup path that relinquishes
    /// intent ownership must make the same exactly-once release. A previously
    /// rejected cleanup may complete only when the command explicitly signals
    /// global credential retirement and this transaction independently proves
    /// the exact activation/policy/advanced-credential lineage.
    async fn complete_grant_activation_cleanup(
        &self,
        command: CompleteGrantActivationCleanup,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError>;

    async fn complete_invalidated_grant_activation(
        &self,
        command: CompleteInvalidatedGrantActivation,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError>;

    async fn provider_grant_receipt(
        &self,
        grant_id: &SponsoredGrantId,
    ) -> Result<Option<ProviderGrantReceipt>, SponsorshipStoreError>;

    async fn grant(
        &self,
        id: &SponsoredGrantId,
    ) -> Result<Option<SponsoredGrant>, SponsorshipStoreError>;

    /// List grants visible to their contributor, newest first by immutable
    /// `(created_at, id)`.
    async fn grants_for_contributor(
        &self,
        contributor: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<SponsoredGrant>, SponsorshipStoreError>;

    /// List grants controlled by their sponsoring owner, newest first by
    /// immutable `(created_at, id)`. The owner index must be transactionally
    /// bound to the grant's policy owner when the grant is created.
    async fn grants_for_sponsor(
        &self,
        sponsor: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<SponsoredGrant>, SponsorshipStoreError>;

    async fn grant_for_launch(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredGrant>, SponsorshipStoreError>;

    async fn usage_reservation(
        &self,
        reservation_id: &SponsoredUsageReservationId,
    ) -> Result<Option<SponsoredUsageReservation>, SponsorshipStoreError>;

    /// Resolve caller idempotency aliases before dynamic authority checks and
    /// replace a matching row's aliases with the complete bounded candidates.
    async fn usage_reservation_for_request(
        &self,
        request_identity: &SponsorshipOperationalPseudonymSet,
    ) -> Result<Option<SponsoredUsageReservation>, SponsorshipStoreError>;

    /// CAS replacement must also verify that allocation and settled usage
    /// still match the candidate; usage reconciliation may update accounting
    /// without changing the owner-facing capability generation.
    async fn replace_grant(
        &self,
        expected_generation: u64,
        grant: &SponsoredGrant,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredGrant, SponsorshipStoreError>;

    /// Atomically marks a grant terminal, releases its concurrency slot and
    /// only the unused aggregate allocation not held by `Reserved` or
    /// `Ambiguous` usage, and appends the audit. Outstanding maximum holds stay
    /// reserved until reconciliation. A later terminal-grant settlement or
    /// release must consume or release that hold atomically and exactly once.
    /// `Expired` additionally requires that authoritative database time has
    /// reached the stored provider-authority expiry; caller/service time may
    /// request reconciliation but cannot authorize early release.
    async fn release_grant_capacity(
        &self,
        expected_generation: u64,
        grant: &SponsoredGrant,
        reason: GrantCapacityRelease,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredGrant, SponsorshipStoreError>;

    /// Return an existing stable request by its caller-input idempotency
    /// fingerprint before applying current gate, clock, pricing, or grant
    /// availability checks. New reservations must atomically validate every
    /// supplied authority fence and hold the quoted maximum. The supplied
    /// credential generation must match grant provenance, not the current root
    /// generation after a successful rotation; a non-active current credential
    /// still rejects new spend. Grant/policy expiry and rate-window admission
    /// must use authoritative database time in the reservation transaction,
    /// never `ReserveSponsoredUsage.now`.
    async fn reserve_usage(
        &self,
        command: ReserveSponsoredUsage,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredUsageReservationOutcome, SponsorshipStoreError>;

    /// Atomically resolve a reservation and update grant/policy accounting.
    /// Before observing clock or pricing, the service returns a durable
    /// terminal reservation only when the request fingerprint, outcome, and
    /// caller-reported quota semantically match that stored result; conflicting
    /// terminal replays fail without mutation.
    /// Active grants convert actual usage from reserved to consumed capacity.
    /// Terminal grants release the reservation's entire maximum hold while
    /// recording any confirmed actual usage. If settlement exhausts any grant
    /// dimension, terminalization must retain all other unresolved holds and
    /// release only the remainder.
    async fn reconcile_usage(
        &self,
        command: ReconcileSponsoredUsage,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredUsageReservation, SponsorshipStoreError>;

    /// Bounded internal discovery surfaces. Each call returns at most one page
    /// and performs no provider, billing, or lifecycle mutation.
    async fn credential_recovery_candidates(
        &self,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsoredCredentialId>, SponsorshipStoreError>;

    async fn activation_recovery_candidates(
        &self,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<LaunchId>, SponsorshipStoreError>;

    /// Discover provider-revocation work and nonterminal grants due for
    /// expiry at or before one fixed CMA timestamp. Continuation cursors must
    /// be bound to `as_of`; changing the clock fence starts a new traversal.
    async fn grant_recovery_candidates(
        &self,
        as_of: &str,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsoredGrantId>, SponsorshipStoreError>;

    async fn usage_recovery_candidates(
        &self,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsoredUsageReservationId>, SponsorshipStoreError>;

    /// Select records still indexed under one retiring key without decrypting
    /// unrelated rows. Rewrap must update ciphertext envelopes and their key
    /// index with one exact-document CAS while preserving every other stored
    /// field. A candidate whose raw document fingerprint no longer matches
    /// returns `false`; it must not overwrite or rewrap the newer mutation.
    async fn ciphertext_rewrap_candidates(
        &self,
        kind: SponsorshipCiphertextRecordKind,
        retiring_key_id: &str,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsorshipCiphertextRewrapCandidate>, SponsorshipStoreError>;

    async fn rewrap_ciphertext(
        &self,
        candidate: &SponsorshipCiphertextRewrapCandidate,
    ) -> Result<bool, SponsorshipStoreError>;

    async fn audit_events(
        &self,
        actor_digest: &str,
        limit: usize,
    ) -> Result<Vec<SponsoredAuditEvent>, SponsorshipStoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SponsoredProviderError {
    #[error("the provider rejected the credential or grant")]
    Rejected,
    #[error("the provider operation timed out before a result was known")]
    Ambiguous,
    /// The adapter proved that dispatch did not occur and no external effect is
    /// possible. Once dispatch might have happened, timeouts, transport loss,
    /// malformed responses, and all other uncertainty are `Ambiguous` (or
    /// `MalformedEvidence` when evidence was returned but is unusable).
    #[error("the provider was unavailable before dispatch; no external effect occurred")]
    Unavailable,
    #[error("the provider returned malformed capability evidence")]
    MalformedEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderOperationResolution<T> {
    Confirmed(T),
    Rejected,
    Pending,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderGrantReceipt {
    pub provider_grant_id: SensitiveProviderValue,
    pub provider_state_version: u64,
    pub expires_at: String,
    pub revocation_handle: SensitiveProviderValue,
    pub issued_at: String,
}

/// Raw provider ownership evidence returned only to the trusted sponsorship
/// service. The service derives the deployment-keyed durable pseudonym; an
/// adapter cannot supply an unkeyed digest as persistence authority.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderCredentialEvidence {
    pub provider: String,
    pub provider_policy_key: String,
    pub provider_policy_handle: SensitiveProviderValue,
    pub provider_credential_id: SensitiveProviderValue,
    pub state_version: u64,
    pub ownership_verified_at: String,
    pub supports_expiry: bool,
    pub supports_revocation: bool,
}

impl fmt::Debug for ProviderCredentialEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderCredentialEvidence")
            .field("provider", &self.provider)
            .field("provider_policy_key", &self.provider_policy_key)
            .field("provider_policy_handle", &"<redacted>")
            .field("provider_credential_id", &"<redacted>")
            .field("state_version", &self.state_version)
            .field("ownership_verified_at", &self.ownership_verified_at)
            .field("supports_expiry", &self.supports_expiry)
            .field("supports_revocation", &self.supports_revocation)
            .finish()
    }
}

impl fmt::Debug for ProviderGrantReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderGrantReceipt")
            .field("provider_grant_id", &"<redacted>")
            .field("provider_state_version", &self.provider_state_version)
            .field("expires_at", &self.expires_at)
            .field("revocation_handle", &"<redacted>")
            .field("issued_at", &self.issued_at)
            .finish()
    }
}

#[async_trait]
pub trait SponsoredCredentialProvider: Send + Sync {
    /// Definitively validate registration ownership. Malformed evidence here is
    /// a rejecting result because no provider mutation has been dispatched.
    async fn validate_ownership(
        &self,
        owner: &SponsorBillingOwner,
        provider: &str,
        provider_policy_key: &str,
        credential: &ProviderCredentialSecret,
    ) -> Result<ProviderCredentialEvidence, SponsoredProviderError>;

    async fn issue_grant(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        policy: &SponsorshipPolicy,
        launch_id: &LaunchId,
        session_id: &SessionId,
    ) -> Result<ProviderGrantReceipt, SponsoredProviderError>;

    /// Read back an issuance whose response was lost. This method must not
    /// create a second provider grant. `Pending` means the same deterministic
    /// operation remains unresolved and its capacity must stay reserved.
    async fn reconcile_grant_issue(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        policy: &SponsorshipPolicy,
        launch_id: &LaunchId,
        session_id: &SessionId,
    ) -> Result<ProviderOperationResolution<ProviderGrantReceipt>, SponsoredProviderError>;

    /// Revoke a provider grant that was durably issued but never committed as a
    /// CMA grant because a final authority fence changed.
    async fn revoke_uncommitted_grant(
        &self,
        stable_request_id: &str,
        receipt: &ProviderGrantReceipt,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
    ) -> Result<(), SponsoredProviderError>;

    async fn reconcile_uncommitted_grant_revocation(
        &self,
        stable_request_id: &str,
        receipt: &ProviderGrantReceipt,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
    ) -> Result<ProviderOperationResolution<()>, SponsoredProviderError>;

    /// Atomically validate the replacement and retire the predecessor root
    /// credential, or leave it usable when the result is not confirmed.
    /// Confirmation must preserve every already-issued, independently
    /// expiring/revocable grant. Credential generations on those grants are
    /// issuance provenance, not a live rotation fence. Providers whose root
    /// rotation invalidates issued grants cannot satisfy this adapter contract.
    /// The returned authority remains in the same durable
    /// `SponsoredCredentialId` lineage: the replacement must manage, read back,
    /// and revoke grants and deterministic operations issued by every earlier
    /// generation. Providers that cannot prove lineage continuity must not
    /// implement or enable this adapter.
    async fn rotate_credential(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        current_secret: &ProviderCredentialSecret,
        replacement_secret: &ProviderCredentialSecret,
    ) -> Result<ProviderCredentialEvidence, SponsoredProviderError>;

    async fn reconcile_credential_rotation(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        current_secret: &ProviderCredentialSecret,
        replacement_secret: &ProviderCredentialSecret,
    ) -> Result<ProviderOperationResolution<ProviderCredentialEvidence>, SponsoredProviderError>;

    /// A successful return must globally invalidate the entire durable
    /// `SponsoredCredentialId` lineage: every grant and deterministic in-flight
    /// operation authorized by any earlier, current, or staged generation, not
    /// merely the supplied secret values. The store retires material only after
    /// this result, or after the same guarantee is returned by read-back below.
    async fn revoke_credential(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        staged_replacement: Option<&ProviderCredentialSecret>,
        superseded_rotation: Option<&PendingProviderOperation>,
    ) -> Result<(), SponsoredProviderError>;

    /// Confirmation means every grant and deterministic in-flight operation in
    /// the durable credential lineage is unusable. This global guarantee is
    /// what permits encrypted material retirement. Definitive rejection leaves
    /// the credential fail-closed in `RevocationPending` with material retained
    /// solely for the owner recovery workflow tracked by #675; it must not
    /// reactivate new spend.
    async fn reconcile_credential_revocation(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        staged_replacement: Option<&ProviderCredentialSecret>,
        superseded_rotation: Option<&PendingProviderOperation>,
    ) -> Result<ProviderOperationResolution<()>, SponsoredProviderError>;

    async fn revoke_grant(
        &self,
        stable_request_id: &str,
        grant: &SponsoredGrant,
        provider_receipt: &ProviderGrantReceipt,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
    ) -> Result<(), SponsoredProviderError>;

    async fn reconcile_grant_revocation(
        &self,
        stable_request_id: &str,
        grant: &SponsoredGrant,
        provider_receipt: &ProviderGrantReceipt,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
    ) -> Result<ProviderOperationResolution<()>, SponsoredProviderError>;
}

/// Read-only billing eligibility observation. Deliberately carries no billing
/// operation identifier, hold, receipt, or compensation handle because this
/// port is forbidden from creating external billing state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingAuthorization {
    /// The sponsor remains eligible. This is an observation, never a billing
    /// hold, authorization, charge, reservation, or other external mutation.
    Confirmed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SponsoredBillingError {
    #[error("billing confirmation timed out")]
    Ambiguous,
    #[error("billing is temporarily unavailable")]
    Unavailable,
}

#[async_trait]
pub trait SponsoredBillingProvider: Send + Sync {
    /// Read-only sponsor eligibility check before provider grant issuance.
    ///
    /// Implementations MUST NOT create or change any external billing state:
    /// no holds, authorizations, charges, invoices, reservations, or pending
    /// operations. `stable_request_id` is correlation/idempotency identity for
    /// reading the same eligibility decision only. Sponsored cost is accounted
    /// exclusively through trusted usage reconciliation. This non-mutation
    /// guarantee is why provider rejection or final authority invalidation
    /// requires no billing compensation workflow.
    async fn confirm_grant(
        &self,
        stable_request_id: &str,
        policy: &SponsorshipPolicy,
        launch_id: &LaunchId,
    ) -> Result<BillingAuthorization, SponsoredBillingError>;

    /// Read back the same non-mutating eligibility decision after an ambiguous
    /// response. This method is also read-only.
    async fn reconcile_grant(
        &self,
        stable_request_id: &str,
        policy: &SponsorshipPolicy,
        launch_id: &LaunchId,
    ) -> Result<ProviderOperationResolution<BillingAuthorization>, SponsoredBillingError>;
}

pub trait SponsorshipClock: Send + Sync {
    fn now(&self) -> String;
}

pub struct SystemSponsorshipClock;

impl SponsorshipClock for SystemSponsorshipClock {
    fn now(&self) -> String {
        crate::rfc3339_at(std::time::SystemTime::now())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SponsoredPricingError {
    #[error("sponsored pricing is unavailable")]
    Unavailable,
    #[error("the provider/model/resource has no trusted sponsored price")]
    Unsupported,
    #[error("the pricing service returned malformed bounds")]
    Malformed,
}

#[async_trait]
pub trait SponsoredPricingProvider: Send + Sync {
    async fn quote(
        &self,
        provider: &str,
        model: &str,
        resource: &str,
        maximum_quota: SponsoredQuota,
        observed_at: &str,
    ) -> Result<SponsoredPriceQuote, SponsoredPricingError>;

    /// Price exactly `actual_quota`. A returned summary whose quota differs is
    /// malformed and must be rejected by the service before persistence.
    async fn price_actual(
        &self,
        quote: &SponsoredPriceQuote,
        actual_quota: SponsoredQuota,
        observed_at: &str,
    ) -> Result<SponsoredUsageSummary, SponsoredPricingError>;
}
