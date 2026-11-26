use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ids::{SandboxId, ServiceId};
use crate::profiles::store::{
    ProfileStoreError, ProfileUseRecorder, SandboxProfileStore, WorkspaceProfileStore,
};
use crate::profiles::SandboxProfileRef;
use crate::{now_rfc3339, Caller, Capability, StoreError};

use super::store::{Page, PageRequest, SandboxServiceStore, SandboxStoreError, SandboxTransaction};
use super::{
    Sandbox, SandboxCredentialPrerequisite, SandboxEvent, SandboxEventKind, SandboxStatus,
};

pub const DEFAULT_OWNER_SANDBOX_LIMIT: u16 = 10;
pub const DEFAULT_VISITOR_SANDBOX_LIMIT: u16 = 1;
pub const DEFAULT_RETENTION_GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const DEFAULT_INACTIVITY_PAUSE_AFTER_SECONDS: u64 = 15 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxOrigin {
    Manual,
    Trigger { launch_id: String },
    Lifecycle { operation_id: String },
}

impl SandboxOrigin {
    /// Stable admission namespace and identity; manual requests have no replay origin.
    pub fn replay_key(&self) -> Option<(&'static str, &str)> {
        match self {
            Self::Manual => None,
            Self::Trigger { launch_id } => Some(("trigger", launch_id)),
            Self::Lifecycle { operation_id } => Some(("lifecycle", operation_id)),
        }
    }

    pub(super) fn validate(&self) -> Result<(), SandboxServiceError> {
        if let Some((_, id)) = self.replay_key() {
            if id.trim().is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
                return Err(SandboxServiceError::InvalidOrigin);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSandbox {
    pub profile: SandboxProfileRef,
    pub name: Option<String>,
    pub confirmed_digest: Option<String>,
    pub origin: SandboxOrigin,
}

/// Narrow lifecycle policy translated from deployment settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub max_sandboxes_per_owner: u16,
    pub max_sandboxes_per_visitor: u16,
    pub inactivity_pause_after_seconds: u64,
    pub checkpoint_interval_seconds: u64,
    pub retention_grace: Duration,
    pub baseline_services: Vec<ServiceId>,
}

/// Current policy without coupling sandbox lifecycle logic to admin storage.
///
/// A fixed policy is itself a source for tests and local composition. Production
/// composition can translate each live platform-settings read into this type.
#[async_trait::async_trait]
pub trait SandboxPolicySource: Send + Sync {
    async fn current_sandbox_policy(&self) -> Result<SandboxPolicy, StoreError>;

    /// Whether the automatic pause reconciler may submit decisions at this
    /// instant. Fixed test policies retain the historical enabled behavior;
    /// deployment policy sources can impose a durable enablement grace period.
    async fn automatic_pause_allowed(&self, _now: &str) -> Result<bool, StoreError> {
        Ok(true)
    }
}

#[async_trait::async_trait]
impl SandboxPolicySource for SandboxPolicy {
    async fn current_sandbox_policy(&self) -> Result<SandboxPolicy, StoreError> {
        Ok(self.clone())
    }
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            max_sandboxes_per_owner: DEFAULT_OWNER_SANDBOX_LIMIT,
            max_sandboxes_per_visitor: DEFAULT_VISITOR_SANDBOX_LIMIT,
            inactivity_pause_after_seconds: DEFAULT_INACTIVITY_PAUSE_AFTER_SECONDS,
            checkpoint_interval_seconds: crate::platform::DEFAULT_CHECKPOINT_INTERVAL_SECONDS,
            retention_grace: DEFAULT_RETENTION_GRACE,
            baseline_services: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxServiceError {
    #[error("account deletion is in progress")]
    AccountDeletionInProgress,
    #[error("renewable Sandbox credential setup is required")]
    CredentialSetupRequired,
    #[error("required connections have not been verified")]
    ConnectionsRequired { services: Vec<ServiceId> },
    #[error("forbidden")]
    Forbidden,
    #[error("Always on requires approval by an administrator")]
    AlwaysOnApprovalRequired,
    #[error("sandbox profile not found")]
    ProfileNotFound,
    #[error("sandbox not found")]
    NotFound,
    #[error("trust confirmation required for digest {commands_digest}")]
    TrustConfirmationRequired { commands_digest: String },
    #[error("sandbox limit reached: {live} of {limit}")]
    SandboxLimitReached {
        limit: u16,
        live: u16,
        usage: super::SandboxQuotaUsage,
    },
    #[error("global sandbox capacity is unavailable")]
    GlobalCapacityReached,
    #[error("model budget refused the commitment: {0}")]
    Budget(crate::budget::BudgetError),
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
    #[error("sandbox name is invalid")]
    InvalidName,
    #[error("sandbox name is already in use")]
    NameConflict,
    #[error("sandbox origin is invalid")]
    InvalidOrigin,
    #[error("sandbox trigger origin is already bound to another request")]
    SandboxOriginConflict,
    #[error("sandbox is not in a deletable state")]
    InvalidState,
    #[error("sandbox service access request is invalid")]
    InvalidServiceAccess,
    #[error("new service access requires a fully paused sandbox")]
    ServiceAccessRequiresPause,
    #[error("sandbox data is not usable: {0}")]
    Corrupt(String),
    #[error("sandbox dependency is unavailable: {0}")]
    Unavailable(String),
}

impl SandboxServiceError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::AccountDeletionInProgress => "account_deletion_in_progress",
            Self::CredentialSetupRequired => "sandbox_credential_setup_required",
            Self::ConnectionsRequired { .. } => "connections_required",
            Self::Forbidden => "forbidden",
            Self::AlwaysOnApprovalRequired => "always_on_approval_required",
            Self::ProfileNotFound => "sandbox_profile_not_found",
            Self::NotFound => "sandbox_not_found",
            Self::TrustConfirmationRequired { .. } => "trust_confirmation_required",
            Self::SandboxLimitReached { .. } => "sandbox_limit_reached",
            Self::GlobalCapacityReached => "sandbox_capacity_unavailable",
            Self::Budget(reason) => reason.code(),
            Self::AdmissionPaused => "admission_paused",
            Self::InvalidName => "invalid_sandbox_name",
            Self::NameConflict => "sandbox_name_conflict",
            Self::InvalidOrigin => "invalid_sandbox_origin",
            Self::SandboxOriginConflict => "sandbox_origin_conflict",
            Self::InvalidState => "sandbox_invalid_state",
            Self::InvalidServiceAccess => "invalid_sandbox_service_access",
            Self::ServiceAccessRequiresPause => "sandbox_service_access_requires_pause",
            Self::Corrupt(_) => "stored_sandbox_corrupt",
            Self::Unavailable(_) => "sandbox_service_unavailable",
        }
    }
}

pub struct SandboxService {
    pub(super) store: Arc<dyn SandboxServiceStore>,
    pub(super) profiles: Arc<dyn SandboxProfileStore>,
    pub(super) workspace_profiles: Arc<dyn WorkspaceProfileStore>,
    pub(super) profile_uses: Arc<dyn ProfileUseRecorder>,
    pub(super) policy: Arc<dyn SandboxPolicySource>,
    pub(super) credentials: Arc<dyn SandboxCredentialPrerequisite>,
    pub(super) platform_spend: std::sync::OnceLock<Arc<dyn crate::budget::PlatformSpendLedger>>,
}

impl SandboxService {
    pub fn new<S, P>(
        store: Arc<S>,
        profiles: Arc<P>,
        policy: SandboxPolicy,
        credentials: Arc<dyn SandboxCredentialPrerequisite>,
    ) -> Arc<Self>
    where
        S: SandboxServiceStore + 'static,
        P: SandboxProfileStore + WorkspaceProfileStore + ProfileUseRecorder + 'static,
    {
        Self::with_policy_source(store, profiles, Arc::new(policy), credentials)
    }

    pub fn with_policy_source<S, P>(
        store: Arc<S>,
        profiles: Arc<P>,
        policy: Arc<dyn SandboxPolicySource>,
        credentials: Arc<dyn SandboxCredentialPrerequisite>,
    ) -> Arc<Self>
    where
        S: SandboxServiceStore + 'static,
        P: SandboxProfileStore + WorkspaceProfileStore + ProfileUseRecorder + 'static,
    {
        let sandbox_profiles: Arc<dyn SandboxProfileStore> = profiles.clone();
        let workspace_profiles: Arc<dyn WorkspaceProfileStore> = profiles.clone();
        let profile_uses: Arc<dyn ProfileUseRecorder> = profiles;
        Self::with_dependencies(
            store,
            sandbox_profiles,
            workspace_profiles,
            profile_uses,
            policy,
            credentials,
        )
    }

    pub(crate) fn with_dependencies<S>(
        store: Arc<S>,
        profiles: Arc<dyn SandboxProfileStore>,
        workspace_profiles: Arc<dyn WorkspaceProfileStore>,
        profile_uses: Arc<dyn ProfileUseRecorder>,
        policy: Arc<dyn SandboxPolicySource>,
        credentials: Arc<dyn SandboxCredentialPrerequisite>,
    ) -> Arc<Self>
    where
        S: SandboxServiceStore + 'static,
    {
        Arc::new(Self {
            store,
            profiles,
            workspace_profiles,
            profile_uses,
            policy,
            credentials,
            platform_spend: std::sync::OnceLock::new(),
        })
    }

    /// Revival reads the platform admission state at the service entry; it
    /// carries no model liability and the turn admission behind it is exact.
    pub fn attach_platform_spend(&self, spend: Arc<dyn crate::budget::PlatformSpendLedger>) {
        let _ = self.platform_spend.set(spend);
    }

    pub(super) async fn require_platform_open(
        &self,
        cause: crate::admission::AdmissionCause,
    ) -> Result<(), crate::admission::AdmissionRefusal> {
        let Some(spend) = self.platform_spend.get() else {
            return Ok(());
        };
        let now = crate::seconds_from_rfc3339(&crate::now_rfc3339())
            .ok_or(crate::budget::BudgetError::Unavailable)?;
        match spend
            .admission_state(now)
            .await?
            .commitment_refusal_for(cause)
        {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    }

    pub async fn list_own(
        &self,
        caller: &Caller,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxServiceError> {
        require(caller, Capability::UseOwnSandboxes)?;
        self.store
            .list_sandboxes(&caller.subject, page)
            .await
            .map_err(store_error)
    }

    pub async fn read(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<Sandbox, SandboxServiceError> {
        require(caller, Capability::UseOwnSandboxes)?;
        self.owned(caller, id).await
    }

    pub async fn events(
        &self,
        caller: &Caller,
        id: &SandboxId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<SandboxEvent>, SandboxServiceError> {
        require(caller, Capability::UseOwnSandboxes)?;
        self.owned(caller, id).await?;
        self.store
            .sandbox_events(id, after, limit)
            .await
            .map_err(store_error)
    }

    /// Give up an existing Always on exception. Only administrators may approve
    /// one; an unset or already disabled preference remains unchanged.
    pub async fn set_always_on(
        &self,
        caller: &Caller,
        id: &SandboxId,
        expected_revision: u64,
        always_on: bool,
    ) -> Result<Sandbox, SandboxServiceError> {
        require(caller, Capability::UseOwnSandboxes)?;
        let current = self.owned(caller, id).await?;
        if always_on {
            return Err(SandboxServiceError::AlwaysOnApprovalRequired);
        }
        if current.revision != expected_revision
            || matches!(
                current.status,
                SandboxStatus::Retiring | SandboxStatus::Retired
            )
        {
            return Err(SandboxServiceError::InvalidState);
        }
        if current.profile.always_on != Some(true) {
            return Ok(current);
        }
        let mut next = current.clone();
        next.profile.always_on = Some(false);
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(SandboxServiceError::InvalidState)?;
        next.updated_at = now_rfc3339();
        next.validate()
            .map_err(|_| SandboxServiceError::InvalidState)?;
        self.store
            .save_sandbox(expected_revision, next.clone())
            .await
            .map_err(store_error)?;
        Ok(next)
    }

    pub async fn delete(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<Sandbox, SandboxServiceError> {
        require(caller, Capability::DeleteOwnSandbox)?;
        let mut sandbox = self.owned(caller, id).await?;
        if matches!(
            sandbox.status,
            SandboxStatus::Retiring | SandboxStatus::Retired
        ) {
            return Ok(sandbox);
        }
        let previous = sandbox.status;
        let expected = sandbox.revision;
        let now = now_rfc3339();
        sandbox
            .begin_retirement(now.clone())
            .map_err(|_| SandboxServiceError::InvalidState)?;
        let committed = self
            .store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: expected,
                sandbox: Box::new(sandbox.clone()),
                event_at: now,
                event: SandboxEventKind::StatusChanged {
                    from: previous,
                    to: SandboxStatus::Retiring,
                },
            })
            .await;
        match committed {
            Ok(()) => Ok(sandbox),
            Err(SandboxStoreError::Conflict) => {
                let current = self.owned(caller, id).await?;
                if matches!(
                    current.status,
                    SandboxStatus::Retiring | SandboxStatus::Retired
                ) {
                    Ok(current)
                } else {
                    Err(SandboxServiceError::Unavailable(
                        "sandbox changed while retirement was requested".into(),
                    ))
                }
            }
            Err(error) => Err(store_error(error)),
        }
    }

    pub(super) async fn owned(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<Sandbox, SandboxServiceError> {
        self.store
            .get_sandbox(id)
            .await
            .map_err(store_error)?
            .filter(|sandbox| sandbox.owner == caller.subject)
            .ok_or(SandboxServiceError::NotFound)
    }
}

pub(super) fn require(caller: &Caller, capability: Capability) -> Result<(), SandboxServiceError> {
    caller
        .authorization
        .allows(capability)
        .then_some(())
        .ok_or(SandboxServiceError::Forbidden)
}

pub(super) fn profile_error(error: ProfileStoreError) -> SandboxServiceError {
    match error {
        ProfileStoreError::NotFound => SandboxServiceError::ProfileNotFound,
        ProfileStoreError::Corrupt(detail) => SandboxServiceError::Corrupt(detail),
        other => SandboxServiceError::Unavailable(other.to_string()),
    }
}

pub(super) fn policy_error(error: StoreError) -> SandboxServiceError {
    match error {
        StoreError::Malformed(detail) => SandboxServiceError::Corrupt(detail),
        other => SandboxServiceError::Unavailable(other.to_string()),
    }
}

pub(super) fn store_error(error: SandboxStoreError) -> SandboxServiceError {
    match error {
        SandboxStoreError::AccountDeletionInProgress => {
            SandboxServiceError::AccountDeletionInProgress
        }
        SandboxStoreError::NotFound => SandboxServiceError::NotFound,
        SandboxStoreError::Duplicate => SandboxServiceError::NameConflict,
        SandboxStoreError::LimitReached { limit, live, usage } => {
            SandboxServiceError::SandboxLimitReached { limit, live, usage }
        }
        SandboxStoreError::GlobalCapacityReached => SandboxServiceError::GlobalCapacityReached,
        SandboxStoreError::Budget(reason) => SandboxServiceError::Budget(reason),
        SandboxStoreError::AdmissionPaused => SandboxServiceError::AdmissionPaused,
        SandboxStoreError::SandboxOriginConflict => SandboxServiceError::SandboxOriginConflict,
        SandboxStoreError::Corrupt(detail) => SandboxServiceError::Corrupt(detail),
        SandboxStoreError::InvalidPage => SandboxServiceError::InvalidState,
        other => SandboxServiceError::Unavailable(other.to_string()),
    }
}
