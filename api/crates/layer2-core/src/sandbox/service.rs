use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ids::{SandboxId, ServiceId};
use crate::profiles::store::{
    ProfileStoreError, ProfileUseRecorder, SandboxProfileStore, WorkspaceProfileStore,
};
use crate::profiles::{
    MarketplaceLocator, ModelAccess, ProfileVisibility, SandboxProfileRef, SandboxProfileSnapshot,
};
use crate::{now_rfc3339, Caller, Capability, StoreError};

use super::service_trust::{
    nested_workspaces_require_trust, validate_sandbox_profile, validate_sandbox_revision,
};
use super::store::{
    Page, PageRequest, SandboxAdmission, SandboxAdmissionName, SandboxServiceStore,
    SandboxStoreError, SandboxTransaction,
};
use super::{
    Sandbox, SandboxCredentialState, SandboxEvent, SandboxEventKind, SandboxLimits,
    SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep,
};

pub const DEFAULT_OWNER_SANDBOX_LIMIT: u16 = 3;
pub const DEFAULT_VISITOR_SANDBOX_LIMIT: u16 = 1;
pub const DEFAULT_RETENTION_GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_SANDBOX_NAME_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxOrigin {
    Manual,
    Trigger { launch_id: String },
}

impl SandboxOrigin {
    fn validate(&self) -> Result<(), SandboxServiceError> {
        if let Self::Trigger { launch_id } = self {
            if launch_id.trim().is_empty() || launch_id.len() > 128 {
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
            retention_grace: DEFAULT_RETENTION_GRACE,
            baseline_services: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxServiceError {
    #[error("forbidden")]
    Forbidden,
    #[error("sandbox profile not found")]
    ProfileNotFound,
    #[error("sandbox not found")]
    NotFound,
    #[error("trust confirmation required for digest {commands_digest}")]
    TrustConfirmationRequired { commands_digest: String },
    #[error("sandbox limit reached: {live} of {limit}")]
    SandboxLimitReached { limit: u16, live: u16 },
    #[error("sandbox name is invalid")]
    InvalidName,
    #[error("sandbox name is already in use")]
    NameConflict,
    #[error("sandbox origin is invalid")]
    InvalidOrigin,
    #[error("sandbox is not in a deletable state")]
    InvalidState,
    #[error("sandbox data is not usable: {0}")]
    Corrupt(String),
    #[error("sandbox dependency is unavailable: {0}")]
    Unavailable(String),
}

impl SandboxServiceError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::ProfileNotFound => "sandbox_profile_not_found",
            Self::NotFound => "sandbox_not_found",
            Self::TrustConfirmationRequired { .. } => "trust_confirmation_required",
            Self::SandboxLimitReached { .. } => "sandbox_limit_reached",
            Self::InvalidName => "invalid_sandbox_name",
            Self::NameConflict => "sandbox_name_conflict",
            Self::InvalidOrigin => "invalid_sandbox_origin",
            Self::InvalidState => "sandbox_invalid_state",
            Self::Corrupt(_) => "stored_sandbox_corrupt",
            Self::Unavailable(_) => "sandbox_service_unavailable",
        }
    }
}

pub struct SandboxService {
    store: Arc<dyn SandboxServiceStore>,
    profiles: Arc<dyn SandboxProfileStore>,
    workspace_profiles: Arc<dyn WorkspaceProfileStore>,
    profile_uses: Arc<dyn ProfileUseRecorder>,
    policy: Arc<dyn SandboxPolicySource>,
}

impl SandboxService {
    pub fn new<S, P>(store: Arc<S>, profiles: Arc<P>, policy: SandboxPolicy) -> Arc<Self>
    where
        S: SandboxServiceStore + 'static,
        P: SandboxProfileStore + WorkspaceProfileStore + ProfileUseRecorder + 'static,
    {
        Self::with_policy_source(store, profiles, Arc::new(policy))
    }

    pub fn with_policy_source<S, P>(
        store: Arc<S>,
        profiles: Arc<P>,
        policy: Arc<dyn SandboxPolicySource>,
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
        )
    }

    pub(crate) fn with_dependencies<S>(
        store: Arc<S>,
        profiles: Arc<dyn SandboxProfileStore>,
        workspace_profiles: Arc<dyn WorkspaceProfileStore>,
        profile_uses: Arc<dyn ProfileUseRecorder>,
        policy: Arc<dyn SandboxPolicySource>,
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
        })
    }

    pub async fn create(
        &self,
        caller: &Caller,
        request: CreateSandbox,
    ) -> Result<Sandbox, SandboxServiceError> {
        request.origin.validate()?;
        let capability = match request.origin {
            SandboxOrigin::Manual => Capability::CreateSandbox,
            SandboxOrigin::Trigger { .. } => Capability::LaunchTriggers,
        };
        if !caller.authorization.allows(capability) {
            return Err(SandboxServiceError::Forbidden);
        }
        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(policy_error)?;
        let limit = match request.origin {
            SandboxOrigin::Manual => policy.max_sandboxes_per_owner,
            SandboxOrigin::Trigger { .. } => policy.max_sandboxes_per_visitor,
        };
        if limit == 0 {
            return Err(SandboxServiceError::SandboxLimitReached { limit, live: 0 });
        }
        if request.profile.revision == 0 {
            return Err(SandboxServiceError::ProfileNotFound);
        }
        let profile = self
            .profiles
            .sandbox_profile(&request.profile.id)
            .await
            .map_err(profile_error)?
            .ok_or(SandboxServiceError::ProfileNotFound)?;
        validate_sandbox_profile(&request.profile, &profile).map_err(profile_error)?;
        if profile.retired_at.is_some()
            || (profile.owner != caller.subject && profile.visibility != ProfileVisibility::Public)
        {
            return Err(SandboxServiceError::ProfileNotFound);
        }
        let revision = self
            .profiles
            .sandbox_profile_revision(&request.profile.id, request.profile.revision)
            .await
            .map_err(profile_error)?
            .ok_or(SandboxServiceError::ProfileNotFound)?;
        validate_sandbox_revision(&request.profile, &revision).map_err(profile_error)?;
        let nested_needs_trust = nested_workspaces_require_trust(
            self.workspace_profiles.as_ref(),
            &revision.content.workspace_profiles,
            &caller.subject,
        )
        .await
        .map_err(profile_error)?;
        let needs_trust = (profile.owner != caller.subject
            && (!revision.content.software_install_commands.is_empty()
                || !revision.content.environment_variables.is_empty()))
            || nested_needs_trust;
        if needs_trust && request.confirmed_digest.as_deref() != Some(revision.digest.as_str()) {
            return Err(SandboxServiceError::TrustConfirmationRequired {
                commands_digest: revision.digest.clone(),
            });
        }

        let name = match request.name {
            Some(name) => SandboxAdmissionName::Exact(validate_name(name)?),
            None => SandboxAdmissionName::Allocate(derived_name(&profile.name)),
        };
        let now = now_rfc3339();
        let reaches = revision
            .content
            .nyxid_services
            .iter()
            .chain(&policy.baseline_services)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let sign_in = match revision.content.model_access {
            ModelAccess::ChronoaiManaged => SandboxSignInState::NotRequired,
            ModelAccess::OwnCredentials => SandboxSignInState::Required,
        };
        let sandbox = Sandbox {
            id: SandboxId::generate(),
            owner: caller.subject.clone(),
            name: match &name {
                SandboxAdmissionName::Exact(value) | SandboxAdmissionName::Allocate(value) => {
                    value.clone()
                }
            },
            origin: request.origin,
            profile: SandboxProfileSnapshot::from(&revision),
            status: SandboxStatus::Provisioning,
            failure: None,
            runtime: None,
            credential: SandboxCredentialState {
                reaches,
                key_generation: 0,
                delivered: false,
                current: None,
                pending: None,
                revoke_debt: Vec::new(),
                sign_in,
            },
            work: SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, now.clone()),
            limits: SandboxLimits {
                max_workspaces: revision.content.max_workspaces,
                max_agents_per_workspace: revision.content.max_agents_per_workspace,
            },
            checkpoint: None,
            last_activity_at: now.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
            retired_at: None,
            purge_after: None,
            revision: 1,
        };
        sandbox
            .validate()
            .map_err(|error| SandboxServiceError::Corrupt(error.to_string()))?;
        let admitted = self
            .store
            .admit_sandbox(SandboxAdmission {
                sandbox,
                name,
                live_limit: limit,
                event_at: now,
            })
            .await
            .map_err(store_error)?;
        let locator = MarketplaceLocator::Sandbox(request.profile.id);
        if let Err(error) = self.profile_uses.record_profile_use(&locator).await {
            tracing::warn!(
                profile_id = locator.as_str(),
                error = %error,
                "sandbox profile use was not recorded"
            );
        }
        Ok(admitted)
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

    async fn owned(&self, caller: &Caller, id: &SandboxId) -> Result<Sandbox, SandboxServiceError> {
        self.store
            .get_sandbox(id)
            .await
            .map_err(store_error)?
            .filter(|sandbox| sandbox.owner == caller.subject)
            .ok_or(SandboxServiceError::NotFound)
    }
}

fn require(caller: &Caller, capability: Capability) -> Result<(), SandboxServiceError> {
    caller
        .authorization
        .allows(capability)
        .then_some(())
        .ok_or(SandboxServiceError::Forbidden)
}

fn validate_name(name: String) -> Result<String, SandboxServiceError> {
    if name.trim().is_empty() || name.len() > MAX_SANDBOX_NAME_BYTES || name.contains('\0') {
        Err(SandboxServiceError::InvalidName)
    } else {
        Ok(name)
    }
}

fn derived_name(profile_name: &str) -> String {
    let trimmed = profile_name.trim();
    let mut end = trimmed.len().min(MAX_SANDBOX_NAME_BYTES);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

fn profile_error(error: ProfileStoreError) -> SandboxServiceError {
    match error {
        ProfileStoreError::NotFound => SandboxServiceError::ProfileNotFound,
        ProfileStoreError::Corrupt(detail) => SandboxServiceError::Corrupt(detail),
        other => SandboxServiceError::Unavailable(other.to_string()),
    }
}

fn policy_error(error: StoreError) -> SandboxServiceError {
    match error {
        StoreError::Malformed(detail) => SandboxServiceError::Corrupt(detail),
        other => SandboxServiceError::Unavailable(other.to_string()),
    }
}

fn store_error(error: SandboxStoreError) -> SandboxServiceError {
    match error {
        SandboxStoreError::NotFound => SandboxServiceError::NotFound,
        SandboxStoreError::Duplicate => SandboxServiceError::NameConflict,
        SandboxStoreError::LimitReached { limit, live } => {
            SandboxServiceError::SandboxLimitReached { limit, live }
        }
        SandboxStoreError::Corrupt(detail) => SandboxServiceError::Corrupt(detail),
        SandboxStoreError::InvalidPage => SandboxServiceError::InvalidState,
        other => SandboxServiceError::Unavailable(other.to_string()),
    }
}
