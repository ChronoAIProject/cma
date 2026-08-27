use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::{
    AuthorityProvenance, Confirmation, ConnectedAccount, ConnectionSnapshot, ConnectionState,
    Launch, LaunchEvent, LaunchId, LaunchOverrides, LaunchQuery, LaunchReviewAdmission,
    LaunchState, LaunchStore, PublicTriggerId, PublishedRevision, PublisherAuthority,
    StartAdmissionSnapshot, TriggerAvailability, TriggerDraft, TriggerId, TriggerPage,
    TriggerPageRequest, TriggerPreview, TriggerRevision, TriggerRevisionContent, TriggerRevisionId,
    TriggerRolloutDenial, TriggerRolloutPolicy, TriggerStore, TriggerStoreError,
    TriggerSupportLookup, TriggerSupportSnapshot, TriggerValidationError, WorkerLease,
};
use crate::{Caller, Capability, SubjectId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchAuthority {
    pub connection: ConnectionSnapshot,
    pub provenance: AuthorityProvenance,
    /// Exact commit selected at the Start boundary.
    pub source_sha: String,
    /// Publisher-authored GitHub Actions files visible at the exact source.
    pub workflow_paths: Vec<String>,
}

/// User-confirmed values consumed at the Launch start boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartLaunchCommand {
    pub expected_revision: u64,
    pub confirmation: Confirmation,
    pub acknowledge_workflow_risk: bool,
    pub now: String,
    pub correlation_id: String,
}

/// Explicit owner authorization for one replacement initial-prompt dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverInitialPromptCommand {
    pub expected_revision: u64,
    pub confirmation: Confirmation,
    pub acknowledge_duplicate_execution_risk: bool,
    pub now: String,
    pub correlation_id: String,
}

/// Exact operator authorization for one read-only effect reconciliation pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeEffectReconciliationCommand {
    pub expected_revision: u64,
    pub effect: super::LaunchEffectKind,
    pub intent_key: String,
    pub expected_fence: u64,
    pub evidence_correlation_id: String,
    pub now: String,
    pub correlation_id: String,
}

/// User-confirmed values for opening or updating a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmPullRequestCommand {
    pub expected_revision: u64,
    pub confirmation: Confirmation,
    pub title: Option<String>,
    pub body: Option<String>,
    pub now: String,
    pub correlation_id: String,
}

#[async_trait]
pub trait TriggerAuthorityVerifier: Send + Sync {
    async fn verify_publisher(
        &self,
        subject: &SubjectId,
        content: &TriggerRevisionContent,
        selected_service_id: &str,
    ) -> Result<PublisherAuthority, TriggerServiceError>;

    async fn verify_launch(
        &self,
        subject: &SubjectId,
        revision: &TriggerRevision,
        selected_service_id: &str,
        previous: Option<&ConnectionSnapshot>,
    ) -> Result<LaunchAuthority, TriggerServiceError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriggerAdmissionError {
    #[error("Trigger admission is unavailable: {0}")]
    Unavailable(String),
    #[error("pricing policy is unavailable: {0}")]
    PricingUnavailable(String),
    #[error("the reviewed price is no longer in force")]
    PriceChanged,
    #[error("the reviewed price expired")]
    QuoteExpired,
    #[error("session quota or deployment capacity is exhausted: {0}")]
    QuotaExceeded(String),
    #[error("the pinned runtime does not support required capability {capability}")]
    CapabilityUnsupported { capability: String },
    #[error("the deployment runtime changed after review")]
    RuntimeChanged,
}

/// Server-owned pricing, quota, and pinned-runtime policy for Trigger Launches.
#[async_trait]
pub trait TriggerLaunchAdmission: Send + Sync {
    async fn review(
        &self,
        owner: &SubjectId,
        launch: &Launch,
        now: &str,
    ) -> Result<LaunchReviewAdmission, TriggerAdmissionError>;

    async fn start(
        &self,
        owner: &SubjectId,
        launch: &Launch,
        now: &str,
    ) -> Result<StartAdmissionSnapshot, TriggerAdmissionError>;
}

struct UnavailableTriggerLaunchAdmission;

#[async_trait]
impl TriggerLaunchAdmission for UnavailableTriggerLaunchAdmission {
    async fn review(
        &self,
        _owner: &SubjectId,
        _launch: &Launch,
        _now: &str,
    ) -> Result<LaunchReviewAdmission, TriggerAdmissionError> {
        Err(TriggerAdmissionError::Unavailable(
            "no deployment admission resolver is configured".into(),
        ))
    }

    async fn start(
        &self,
        _owner: &SubjectId,
        _launch: &Launch,
        _now: &str,
    ) -> Result<StartAdmissionSnapshot, TriggerAdmissionError> {
        Err(TriggerAdmissionError::Unavailable(
            "no deployment admission resolver is configured".into(),
        ))
    }
}

/// Public-safe metadata for one reusable contributor connection.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TriggerConnection {
    pub service_id: String,
    pub state_version: i64,
    pub state: ConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<ConnectedAccount>,
    #[serde(default)]
    pub granted_scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_observed_at: Option<String>,
}

/// Browser authorization handoff returned by the credential broker.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TriggerConnectionAuthorization {
    pub service_id: String,
    pub authorization_url: String,
    pub attempt_nonce: String,
}

/// Canonical repository and exact branch head resolved by the provider.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TriggerRepositorySelection {
    pub repository: super::RepositoryIdentity,
    pub source_sha: String,
}

/// Provider-neutral user-connection operations used by the public interface.
#[async_trait]
pub trait TriggerConnectionManager: Send + Sync {
    async fn list_connections(
        &self,
        subject: &SubjectId,
    ) -> Result<Vec<TriggerConnection>, TriggerServiceError>;

    /// Owner-concealing lookup used before reserving a revoke operation.
    ///
    /// The default filters the complete live list. Durable implementations may
    /// override it so a persisted revoke intent or receipt remains discoverable
    /// when the provider's active-only list no longer contains the connection.
    async fn connection_for_revocation(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<Option<TriggerConnection>, TriggerServiceError> {
        Ok(self
            .list_connections(subject)
            .await?
            .into_iter()
            .find(|connection| connection.service_id == service_id))
    }

    /// Reconcile or create the placeholder named by a persisted client intent,
    /// then begin its authorization flow.
    async fn begin_connection(
        &self,
        subject: &SubjectId,
        intent_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError>;

    async fn reconnect(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError>;

    /// Deactivate one exact owner-scoped connection and return its inactive
    /// provider-observed projection. Implementations must make repeats converge
    /// without treating an active-list miss as ownership evidence.
    async fn revoke_connection(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        Err(TriggerServiceError::Authority(
            TriggerAuthorityError::ProviderAuthenticationUnavailable,
        ))
    }

    async fn preflight_connection(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError>;

    async fn resolve_repository(
        &self,
        subject: &SubjectId,
        service_id: &str,
        repository_id: u64,
        selected_ref: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError>;

    /// Resolve a human-facing owner/name locator to immutable provider identity.
    async fn resolve_repository_locator(
        &self,
        subject: &SubjectId,
        service_id: &str,
        owner: &str,
        name: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriggerAuthorityError {
    #[error("the CMA sign-in no longer has usable delegated authority")]
    OwnerCredentialUnavailable,
    #[error("the selected connection does not exist or is unavailable to this subject")]
    ConnectionNotFound,
    #[error("the public workflow requires a personal GitHub connection")]
    ConnectionNotPersonal,
    #[error("the selected connection is inactive")]
    ConnectionInactive,
    #[error("the selected connection must be reauthorized")]
    ReauthorizationRequired,
    #[error("the selected connection does not satisfy required scope class {required}")]
    InsufficientScope { required: String },
    #[error("the selected authority changed since the retained preflight")]
    AuthorityDrift,
    #[error("the provider refused this subject or exact connection")]
    Forbidden,
    #[error("the requested provider resource no longer exists")]
    ResourceNotFound,
    #[error("the provider rate limit was reached")]
    RateLimited { retry_after: Option<String> },
    #[error("the provider is temporarily unavailable")]
    Transient,
    #[error("the provider rejected the typed operation")]
    ProviderRejected,
    #[error("the upstream repository currently does not permit this fork workflow")]
    UpstreamNotForkable,
    #[error("the connected account already uses an incompatible fork in this network")]
    ForkNetworkOccupied,
    #[error("the provider returned malformed authority evidence")]
    MalformedEvidence,
    #[error("GitHub connections require provider-backed CMA authentication")]
    ProviderAuthenticationUnavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum TriggerServiceError {
    #[error("trigger resource was not found")]
    NotFound,
    #[error("the caller does not own this trigger resource")]
    Forbidden,
    #[error("Launch progress cursor {cursor} is beyond the durable event head {head}")]
    CursorBeyondEnd { cursor: u64, head: u64 },
    #[error("trigger input is invalid: {0}")]
    Validation(#[from] TriggerValidationError),
    #[error("trigger persistence failed: {0}")]
    Store(#[from] TriggerStoreError),
    #[error("trigger authority could not be verified: {0}")]
    Authority(#[from] TriggerAuthorityError),
    #[error("trigger launch admission failed: {0}")]
    Admission(#[from] TriggerAdmissionError),
    #[error("trigger rollout refused this operation: {0}")]
    Rollout(#[from] TriggerRolloutDenial),
}

pub struct TriggerService {
    triggers: Arc<dyn TriggerStore>,
    launches: Arc<dyn LaunchStore>,
    authority: Arc<dyn TriggerAuthorityVerifier>,
    launch_admission: Arc<dyn TriggerLaunchAdmission>,
    rollout: Arc<TriggerRolloutPolicy>,
}

impl TriggerService {
    pub fn new(
        triggers: Arc<dyn TriggerStore>,
        launches: Arc<dyn LaunchStore>,
        authority: Arc<dyn TriggerAuthorityVerifier>,
    ) -> Self {
        Self {
            triggers,
            launches,
            authority,
            launch_admission: Arc::new(UnavailableTriggerLaunchAdmission),
            rollout: Arc::new(TriggerRolloutPolicy::allow_all()),
        }
    }

    pub fn with_rollout(mut self, rollout: Arc<TriggerRolloutPolicy>) -> Self {
        self.rollout = rollout;
        self
    }

    pub fn with_launch_admission(mut self, admission: Arc<dyn TriggerLaunchAdmission>) -> Self {
        self.launch_admission = admission;
        self
    }

    pub fn rollout_policy(&self) -> Arc<TriggerRolloutPolicy> {
        Arc::clone(&self.rollout)
    }

    pub async fn create_draft(
        &self,
        owner: SubjectId,
        selected_service_id: String,
        mut content: TriggerRevisionContent,
        now: impl Into<String>,
    ) -> Result<TriggerDraft, TriggerServiceError> {
        self.rollout
            .publisher_admission(&owner, content.upstream.repository.repository_id)?;
        let id = TriggerId::new();
        content.trigger_id = id.clone();
        content.publisher_subject_id = owner.clone();
        content.revision_number = 1;
        content.validate()?;
        let now = now.into();
        let draft = TriggerDraft {
            id,
            owner,
            selected_service_id,
            version: 1,
            next_revision_number: 1,
            content,
            created_at: now.clone(),
            updated_at: now,
        };
        self.triggers.insert_draft(&draft).await?;
        Ok(draft)
    }

    pub async fn get_draft(
        &self,
        owner: &SubjectId,
        id: &TriggerId,
    ) -> Result<TriggerDraft, TriggerServiceError> {
        let draft = self
            .triggers
            .get_draft(id)
            .await?
            .ok_or(TriggerServiceError::NotFound)?;
        require_owner(owner, &draft.owner)?;
        Ok(draft)
    }

    pub async fn list_drafts(
        &self,
        owner: &SubjectId,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerDraft>, TriggerServiceError> {
        Ok(self.triggers.list_drafts_for(owner, page).await?)
    }

    pub async fn update_draft(
        &self,
        owner: &SubjectId,
        id: &TriggerId,
        expected_version: u64,
        selected_service_id: String,
        mut content: TriggerRevisionContent,
        now: impl Into<String>,
    ) -> Result<TriggerDraft, TriggerServiceError> {
        let current = self.get_draft(owner, id).await?;
        if current.version != expected_version {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_version,
                actual: current.version,
            }
            .into());
        }
        self.rollout
            .publisher_admission(owner, content.upstream.repository.repository_id)?;
        content.trigger_id = current.id.clone();
        content.publisher_subject_id = current.owner.clone();
        content.revision_number = current.next_revision_number;
        content.validate()?;
        let mut replacement = current;
        replacement.version = replacement.version.checked_add(1).ok_or_else(|| {
            TriggerValidationError::field("version", "the draft revision counter is exhausted")
        })?;
        replacement.selected_service_id = selected_service_id;
        replacement.content = content;
        replacement.updated_at = now.into();
        Ok(self
            .triggers
            .replace_draft(expected_version, &replacement)
            .await?)
    }

    pub async fn publish(
        &self,
        owner: &SubjectId,
        id: &TriggerId,
        expected_version: u64,
        selected_service_id: &str,
        now: impl Into<String>,
    ) -> Result<PublishedRevision, TriggerServiceError> {
        let mut draft = self.get_draft(owner, id).await?;
        self.rollout
            .publisher_admission(owner, draft.content.upstream.repository.repository_id)?;
        if draft.version != expected_version {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_version,
                actual: draft.version,
            }
            .into());
        }
        if draft.selected_service_id.is_empty() {
            // Backfill the only pre-feature shape on first authorized publish.
            draft.selected_service_id = selected_service_id.to_string();
        } else if draft.selected_service_id != selected_service_id {
            return Err(TriggerValidationError::field(
                "selected_service_id",
                "must match the connection that last verified this draft",
            )
            .into());
        }
        draft.content.revision_number = draft.next_revision_number;
        draft.content.validate()?;
        let verified = self
            .authority
            .verify_publisher(owner, &draft.content, selected_service_id)
            .await?;
        verify_authoritative_identity(&draft.content, &verified)?;
        let published_at = now.into();
        let revision = TriggerRevision {
            id: TriggerRevisionId::new(),
            public_id: PublicTriggerId::new(),
            digest: draft.content.digest()?,
            content: draft.content.clone(),
            publisher_authority: verified,
            availability: TriggerAvailability::Published,
            published_at: published_at.clone(),
            retired_at: None,
        };
        draft.version = draft.version.checked_add(1).ok_or_else(|| {
            TriggerValidationError::field("version", "the draft revision counter is exhausted")
        })?;
        draft.next_revision_number =
            draft.next_revision_number.checked_add(1).ok_or_else(|| {
                TriggerValidationError::field(
                    "next_revision_number",
                    "the published revision counter is exhausted",
                )
            })?;
        draft.content.revision_number = draft.next_revision_number;
        draft.updated_at = published_at;
        Ok(self
            .triggers
            .publish_revision(expected_version, &draft, &revision)
            .await?)
    }

    pub async fn preview(
        &self,
        public_id: &PublicTriggerId,
    ) -> Result<TriggerPreview, TriggerServiceError> {
        let revision = self
            .triggers
            .get_public_revision(public_id)
            .await?
            .ok_or(TriggerServiceError::NotFound)?;
        Ok(TriggerPreview::from(&revision))
    }

    pub async fn get_revision(
        &self,
        owner: &SubjectId,
        id: &TriggerRevisionId,
    ) -> Result<TriggerRevision, TriggerServiceError> {
        let revision = self
            .triggers
            .get_revision(id)
            .await?
            .ok_or(TriggerServiceError::NotFound)?;
        require_owner(owner, &revision.content.publisher_subject_id)?;
        Ok(revision)
    }

    pub async fn list_revisions(
        &self,
        owner: &SubjectId,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerRevision>, TriggerServiceError> {
        Ok(self.triggers.list_revisions_for(owner, page).await?)
    }

    pub async fn retire(
        &self,
        owner: &SubjectId,
        id: &TriggerRevisionId,
        retired_at: &str,
    ) -> Result<TriggerRevision, TriggerServiceError> {
        self.get_revision(owner, id).await?;
        self.triggers
            .retire_revision(id, owner, retired_at)
            .await?
            .ok_or(TriggerServiceError::NotFound)
    }

    pub async fn create_or_resume_launch(
        &self,
        owner: SubjectId,
        public_id: &PublicTriggerId,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<Launch, TriggerServiceError> {
        let now = now.into();
        let correlation_id = correlation_id.into();
        let revision = self
            .triggers
            .get_public_revision(public_id)
            .await?
            .ok_or(TriggerServiceError::NotFound)?;
        if let Some(current) = self
            .launches
            .get_current_launch(&owner, &revision.id)
            .await?
        {
            if current.started_at.is_some()
                || !matches!(
                    current.state,
                    super::LaunchState::Draft | super::LaunchState::AuthRequired
                )
                || !current.draft_is_expired(&now)?
            {
                return Ok(current);
            }
            self.rollout.launch_admission(
                &owner,
                &revision.content.publisher_subject_id,
                revision.content.upstream.repository.repository_id,
            )?;
            let mut expired = current;
            expired.expire_draft(now.clone(), correlation_id.clone())?;
            expired.revision_number = expired.revision_number.checked_add(1).ok_or_else(|| {
                TriggerValidationError::field("revision_number", "the Launch revision is exhausted")
            })?;
            let replacement = self
                .admitted_launch(owner, revision, &now, &correlation_id)
                .await?;
            return Ok(self
                .launches
                .expire_current_and_insert(&expired, &replacement)
                .await?);
        }
        if revision.availability != TriggerAvailability::Published {
            return Err(TriggerValidationError::new("this Trigger revision is retired").into());
        }
        self.rollout.launch_admission(
            &owner,
            &revision.content.publisher_subject_id,
            revision.content.upstream.repository.repository_id,
        )?;
        let launch = self
            .admitted_launch(owner, revision, &now, &correlation_id)
            .await?;
        Ok(self.launches.insert_or_get_current(&launch).await?)
    }

    pub async fn create_new_launch(
        &self,
        owner: SubjectId,
        public_id: &PublicTriggerId,
        idempotency_key_hash: impl Into<String>,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<Launch, TriggerServiceError> {
        let revision = self
            .triggers
            .get_public_revision(public_id)
            .await?
            .ok_or(TriggerServiceError::NotFound)?;
        if revision.availability != TriggerAvailability::Published {
            return Err(TriggerValidationError::new("this Trigger revision is retired").into());
        }
        self.rollout.launch_admission(
            &owner,
            &revision.content.publisher_subject_id,
            revision.content.upstream.repository.repository_id,
        )?;
        let now = now.into();
        let correlation_id = correlation_id.into();
        let mut launch = self
            .admitted_launch(owner.clone(), revision, &now, &correlation_id)
            .await?;
        launch.acknowledge_new_launch(owner, now, idempotency_key_hash, correlation_id)?;
        self.launches.insert_new(&launch).await?;
        Ok(launch)
    }

    pub async fn get_launch(
        &self,
        owner: &SubjectId,
        id: &LaunchId,
    ) -> Result<Launch, TriggerServiceError> {
        let launch = self
            .launches
            .get_launch(id)
            .await?
            .ok_or(TriggerServiceError::NotFound)?;
        require_owner(owner, &launch.owner)?;
        Ok(launch)
    }

    pub async fn list_launches(
        &self,
        owner: &SubjectId,
        public_id: Option<PublicTriggerId>,
        states: Vec<LaunchState>,
        page: TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerServiceError> {
        Ok(self
            .launches
            .list_launches(&LaunchQuery {
                owner: Some(owner.clone()),
                public_id,
                states,
                updated_before: None,
                page,
            })
            .await?)
    }

    pub async fn progress(
        &self,
        owner: &SubjectId,
        id: &LaunchId,
        after: u64,
        limit: usize,
    ) -> Result<Vec<LaunchEvent>, TriggerServiceError> {
        let launch = self.get_launch(owner, id).await?;
        let head = launch.event_high_water();
        if after > head {
            return Err(TriggerServiceError::CursorBeyondEnd {
                cursor: after,
                head,
            });
        }
        let limit = limit.clamp(1, 500);
        // Read the aggregate tail before the canonical collection. During a
        // lazy migration, this ordering prevents a concurrent compaction from
        // making an accepted event temporarily invisible.
        let tail = launch
            .events
            .iter()
            .filter(|event| event.sequence > after)
            .cloned()
            .collect::<Vec<_>>();
        let canonical = self.launches.list_launch_events(id, after, limit).await?;
        let mut ordered = BTreeMap::new();
        for event in tail.into_iter().chain(canonical) {
            if event.launch_id != *id {
                return Err(TriggerStoreError::Malformed(
                    "Launch progress contains an event for another Launch".into(),
                )
                .into());
            }
            match ordered.entry(event.sequence) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(event);
                }
                std::collections::btree_map::Entry::Occupied(entry) if entry.get() == &event => {}
                std::collections::btree_map::Entry::Occupied(_) => {
                    return Err(TriggerStoreError::Malformed(
                        "Launch progress contains a conflicting duplicate sequence".into(),
                    )
                    .into());
                }
            }
        }
        let events = ordered
            .into_values()
            .into_iter()
            .take(limit)
            .collect::<Vec<_>>();
        if after < launch.event_high_water()
            && events.first().map(|event| event.sequence) != after.checked_add(1)
        {
            return Err(TriggerStoreError::Malformed(
                "Launch progress contains a sequence gap".into(),
            )
            .into());
        }
        if events.windows(2).any(|pair| {
            pair[0]
                .sequence
                .checked_add(1)
                .is_none_or(|next| next != pair[1].sequence)
        }) || (events.len() < limit
            && events.last().map_or(after, |event| event.sequence) < launch.event_high_water())
        {
            return Err(TriggerStoreError::Malformed(
                "Launch progress contains a sequence gap".into(),
            )
            .into());
        }
        Ok(events)
    }

    /// Redacted cross-owner support read, gated by the existing narrow activity
    /// capability rather than the broader session-operator grants.
    pub async fn support(
        &self,
        caller: &Caller,
        lookup: TriggerSupportLookup,
    ) -> Result<TriggerSupportSnapshot, TriggerServiceError> {
        caller
            .authorization
            .require(Capability::ReadActivityLog)
            .map_err(|_| TriggerServiceError::Forbidden)?;
        let launch = match lookup {
            TriggerSupportLookup::Launch(id) => self.launches.get_launch(&id).await?,
            TriggerSupportLookup::CorrelationId(correlation_id) => {
                self.launches
                    .find_launch_by_correlation_id(&correlation_id)
                    .await?
            }
        }
        .ok_or(TriggerServiceError::NotFound)?;
        Ok(TriggerSupportSnapshot::from_launch(&launch))
    }

    /// Internal bounded inventory for the in-process Launch worker.
    ///
    /// The store remains authoritative; callers must still acquire each
    /// candidate's lease because this page can be stale immediately.
    pub async fn worker_candidates(
        &self,
        updated_before: Option<String>,
        page: TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerServiceError> {
        Ok(self
            .launches
            .list_worker_candidates(updated_before, &page)
            .await?)
    }

    /// Bounded inventory for short-lived capability renewal and terminal
    /// revocation. Callers still acquire the shared Launch lease before acting.
    pub async fn capability_candidates(
        &self,
        page: TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerServiceError> {
        Ok(self.launches.list_capability_candidates(&page).await?)
    }

    pub async fn cleanup_candidates(
        &self,
        delete_before: &str,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerServiceError> {
        Ok(self
            .launches
            .list_cleanup_candidates(delete_before, page)
            .await?)
    }

    pub async fn workspace_candidates(
        &self,
        page: TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerServiceError> {
        Ok(self.launches.list_workspace_candidates(&page).await?)
    }

    pub async fn update_overrides(
        &self,
        owner: &SubjectId,
        id: &LaunchId,
        expected_revision: u64,
        overrides: LaunchOverrides,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<Launch, TriggerServiceError> {
        let now = now.into();
        let correlation_id = correlation_id.into();
        let mut launch = self.get_launch(owner, id).await?;
        self.fence_pre_start(expected_revision, &mut launch, &now, &correlation_id)
            .await?;
        launch.update_overrides(overrides, now.clone(), correlation_id)?;
        let admission = self.launch_admission.review(owner, &launch, &now).await?;
        launch.apply_review_admission(admission)?;
        persist_user_mutation(self.launches.as_ref(), expected_revision, launch).await
    }

    pub async fn select_connection(
        &self,
        owner: &SubjectId,
        id: &LaunchId,
        expected_revision: u64,
        selected_service_id: &str,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<Launch, TriggerServiceError> {
        let now = now.into();
        let correlation_id = correlation_id.into();
        let mut launch = self.get_launch(owner, id).await?;
        self.fence_pre_start(expected_revision, &mut launch, &now, &correlation_id)
            .await?;
        let verified = self
            .authority
            .verify_launch(owner, &launch.revision, selected_service_id, None)
            .await?;
        launch.select_connection(verified.connection, now, correlation_id)?;
        launch.authority = Some(verified.provenance);
        launch.source_sha = verified.source_sha;
        launch.workflow_paths = verified.workflow_paths;
        launch.workflow_risk_acknowledged = false;
        persist_user_mutation(self.launches.as_ref(), expected_revision, launch).await
    }

    pub async fn start(
        &self,
        owner: &SubjectId,
        id: &LaunchId,
        command: StartLaunchCommand,
    ) -> Result<Launch, TriggerServiceError> {
        let StartLaunchCommand {
            expected_revision,
            confirmation,
            acknowledge_workflow_risk,
            now,
            correlation_id,
        } = command;
        let mut launch = self.get_launch(owner, id).await?;
        self.fence_pre_start(expected_revision, &mut launch, &now, &correlation_id)
            .await?;
        if let Err(denial) = self.rollout.launch_orchestration(&launch) {
            if launch.blocked_reason.is_none() {
                launch.block(
                    super::LaunchBlockedReason::FeaturePaused,
                    now.clone(),
                    correlation_id.clone(),
                )?;
                persist_user_mutation(self.launches.as_ref(), expected_revision, launch).await?;
            }
            return Err(denial.into());
        }
        launch.resume_feature_pause(now.clone(), correlation_id.clone())?;
        let admission = self.launch_admission.start(owner, &launch, &now).await?;
        launch.record_start_admission(admission)?;
        let connection = launch.connection.as_ref().ok_or_else(|| {
            TriggerValidationError::field("connection", "select an eligible connection")
        })?;
        let verified = self
            .authority
            .verify_launch(
                owner,
                &launch.revision,
                &connection.service_id,
                Some(connection),
            )
            .await?;
        if verified.connection.service_id != connection.service_id
            || verified.connection.account.account_id != connection.account.account_id
            || verified.source_sha != launch.source_sha
            || verified.workflow_paths != launch.workflow_paths
        {
            return Err(TriggerAuthorityError::AuthorityDrift.into());
        }
        launch.connection = Some(verified.connection);
        launch.authority = Some(verified.provenance);
        launch.source_sha = verified.source_sha;
        launch.workflow_paths = verified.workflow_paths;
        if acknowledge_workflow_risk && !launch.workflow_paths.is_empty() {
            let mut workflow_confirmation = confirmation.clone();
            workflow_confirmation.action = super::ConfirmationAction::WorkflowRisk;
            launch.acknowledge_workflow_risk(workflow_confirmation, correlation_id.clone())?;
            launch.start(confirmation, now, correlation_id)?;
        } else {
            launch.start(confirmation, now, correlation_id)?;
        }
        persist_user_mutation(self.launches.as_ref(), expected_revision, launch).await
    }

    pub async fn cancel(
        &self,
        owner: &SubjectId,
        id: &LaunchId,
        expected_revision: u64,
        confirmation: Confirmation,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<Launch, TriggerServiceError> {
        let mut launch = self.get_launch(owner, id).await?;
        if launch.revision_number != expected_revision {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: launch.revision_number,
            }
            .into());
        }
        let before = launch.clone();
        launch.cancel(confirmation, now, correlation_id)?;
        if launch == before {
            return Ok(launch);
        }
        persist_user_mutation(self.launches.as_ref(), expected_revision, launch).await
    }

    pub async fn recover_initial_prompt(
        &self,
        owner: &SubjectId,
        id: &LaunchId,
        command: RecoverInitialPromptCommand,
    ) -> Result<Launch, TriggerServiceError> {
        let RecoverInitialPromptCommand {
            expected_revision,
            confirmation,
            acknowledge_duplicate_execution_risk,
            now,
            correlation_id,
        } = command;
        let mut launch = self.get_launch(owner, id).await?;
        launch.authorize_initial_prompt_recovery(
            confirmation,
            acknowledge_duplicate_execution_risk,
            now,
            correlation_id,
        )?;
        persist_user_mutation(self.launches.as_ref(), expected_revision, launch).await
    }

    pub async fn authorize_effect_reconciliation(
        &self,
        caller: &Caller,
        id: &LaunchId,
        command: AuthorizeEffectReconciliationCommand,
    ) -> Result<Launch, TriggerServiceError> {
        caller
            .authorization
            .require(Capability::ControlAnySession)
            .map_err(|_| TriggerServiceError::Forbidden)?;
        let AuthorizeEffectReconciliationCommand {
            expected_revision,
            effect,
            intent_key,
            expected_fence,
            evidence_correlation_id,
            now,
            correlation_id,
        } = command;
        let mut launch = self
            .launches
            .get_launch(id)
            .await?
            .ok_or(TriggerServiceError::NotFound)?;
        launch.authorize_effect_reconciliation(
            effect,
            &intent_key,
            expected_fence,
            &evidence_correlation_id,
            now,
            correlation_id,
        )?;
        persist_user_mutation(self.launches.as_ref(), expected_revision, launch).await
    }

    pub async fn request_diff_review(
        &self,
        owner: &SubjectId,
        id: &LaunchId,
        expected_revision: u64,
        confirmation: Confirmation,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<Launch, TriggerServiceError> {
        let mut launch = self.get_launch(owner, id).await?;
        launch.request_diff_review(confirmation, now, correlation_id)?;
        persist_user_mutation(self.launches.as_ref(), expected_revision, launch).await
    }

    pub async fn confirm_branch_publication(
        &self,
        owner: &SubjectId,
        id: &LaunchId,
        expected_revision: u64,
        confirmation: Confirmation,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<Launch, TriggerServiceError> {
        let mut launch = self.get_launch(owner, id).await?;
        launch.confirm_branch_publication(confirmation, now, correlation_id)?;
        persist_user_mutation(self.launches.as_ref(), expected_revision, launch).await
    }

    pub async fn confirm_pull_request(
        &self,
        owner: &SubjectId,
        id: &LaunchId,
        command: ConfirmPullRequestCommand,
    ) -> Result<Launch, TriggerServiceError> {
        let ConfirmPullRequestCommand {
            expected_revision,
            confirmation,
            title,
            body,
            now,
            correlation_id,
        } = command;
        let mut launch = self.get_launch(owner, id).await?;
        launch.confirm_pull_request(
            confirmation,
            title.as_deref(),
            body.as_deref(),
            now,
            correlation_id,
        )?;
        persist_user_mutation(self.launches.as_ref(), expected_revision, launch).await
    }

    async fn admitted_launch(
        &self,
        owner: SubjectId,
        revision: TriggerRevision,
        now: &str,
        correlation_id: &str,
    ) -> Result<Launch, TriggerServiceError> {
        let mut launch = Launch::new(owner.clone(), revision, now, correlation_id)?;
        let admission = self.launch_admission.review(&owner, &launch, now).await?;
        launch.apply_review_admission(admission)?;
        Ok(launch)
    }

    async fn fence_pre_start(
        &self,
        expected_revision: u64,
        launch: &mut Launch,
        now: &str,
        correlation_id: &str,
    ) -> Result<(), TriggerServiceError> {
        if launch.revision_number != expected_revision {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: launch.revision_number,
            }
            .into());
        }
        if launch.started_at.is_none()
            && matches!(
                launch.state,
                super::LaunchState::Draft | super::LaunchState::AuthRequired
            )
            && launch.draft_is_expired(now)?
        {
            launch.expire_draft(now, correlation_id)?;
            let expired =
                persist_user_mutation(self.launches.as_ref(), expected_revision, launch.clone())
                    .await?;
            *launch = expired;
            return Err(TriggerValidationError::field(
                "draft_expires_at",
                "this Launch draft expired; open the README link again for a New Launch",
            )
            .into());
        }
        Ok(())
    }

    pub async fn acquire_worker(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        holder: &str,
        acquired_at: &str,
        expires_at: &str,
    ) -> Result<Option<Launch>, TriggerServiceError> {
        if holder.trim().is_empty() || expires_at <= acquired_at {
            return Err(TriggerValidationError::field("worker_lease", "is not usable").into());
        }
        Ok(self
            .launches
            .acquire_lease(id, expected_revision, holder, acquired_at, expires_at)
            .await?)
    }

    pub async fn commit_worker(
        &self,
        expected_revision: u64,
        lease: &WorkerLease,
        mut launch: Launch,
    ) -> Result<Launch, TriggerServiceError> {
        let current = launch.lease.as_ref().ok_or_else(|| {
            TriggerValidationError::field("worker_lease", "is missing from the mutation")
        })?;
        if current != lease || lease.fencing_token != launch.fencing_counter {
            return Err(TriggerStoreError::StaleFence {
                expected: lease.fencing_token,
                actual: launch.fencing_counter,
            }
            .into());
        }
        launch.revision_number = expected_revision.checked_add(1).ok_or_else(|| {
            TriggerValidationError::field("revision_number", "the Launch revision is exhausted")
        })?;
        Ok(self
            .launches
            .replace_launch(expected_revision, Some(lease.fencing_token), &launch)
            .await?)
    }

    pub async fn release_worker(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        lease: &WorkerLease,
    ) -> Result<Launch, TriggerServiceError> {
        Ok(self
            .launches
            .release_lease(id, expected_revision, lease)
            .await?)
    }
}

async fn persist_user_mutation(
    store: &dyn LaunchStore,
    expected_revision: u64,
    mut launch: Launch,
) -> Result<Launch, TriggerServiceError> {
    if launch.revision_number != expected_revision {
        return Err(TriggerStoreError::RevisionConflict {
            expected: expected_revision,
            actual: launch.revision_number,
        }
        .into());
    }
    if launch.lease.is_some() {
        return Err(TriggerStoreError::LeaseHeld.into());
    }
    launch.revision_number = expected_revision.checked_add(1).ok_or_else(|| {
        TriggerValidationError::field("revision_number", "the Launch revision is exhausted")
    })?;
    Ok(store
        .replace_launch(expected_revision, None, &launch)
        .await?)
}

fn require_owner(actual: &SubjectId, expected: &SubjectId) -> Result<(), TriggerServiceError> {
    if actual == expected {
        Ok(())
    } else {
        Err(TriggerServiceError::Forbidden)
    }
}

fn verify_authoritative_identity(
    content: &TriggerRevisionContent,
    verified: &PublisherAuthority,
) -> Result<(), TriggerServiceError> {
    let expected = &content.upstream.repository;
    if content.publisher_subject_id != verified.subject
        || expected.repository_id != verified.repository.repository_id
        || expected.repository_node_id != verified.repository.repository_node_id
        || expected.owner_id != verified.repository.owner_id
        || expected.owner_node_id != verified.repository.owner_node_id
        || expected.canonical_owner != verified.repository.canonical_owner
        || expected.canonical_name != verified.repository.canonical_name
        || verified.connection.service_id.trim().is_empty()
    {
        return Err(TriggerAuthorityError::AuthorityDrift.into());
    }
    Ok(())
}
