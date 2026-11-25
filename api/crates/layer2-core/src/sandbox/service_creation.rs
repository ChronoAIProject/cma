use crate::profiles::{
    MarketplaceLocator, ModelAccess, ProfileVisibility, SandboxProfileContent,
    SandboxProfileRevision, SandboxProfileSnapshot,
};
use crate::{now_rfc3339, Caller, Capability, SandboxId, ServiceId, SubjectId};

use super::service::{
    policy_error, profile_error, require, store_error, CreateSandbox, SandboxPolicy,
    SandboxService, SandboxServiceError,
};
use super::service_admission::{exact_name, requested_name};
use super::service_trust::{
    foreign_workspace_setups, validate_sandbox_profile, validate_sandbox_revision,
};
use super::store::{
    CreationFence, SandboxAdmission, SandboxAdmissionName, SandboxAdmissionReplay,
    SandboxAdmissionResult, SandboxLimitClass,
};
use super::{
    InitialServiceConnection, Sandbox, SandboxCredentialSource, SandboxCredentialState,
    SandboxLimits, SandboxOrigin, SandboxSignInState, SandboxStatus, SandboxWorkState,
    SandboxWorkStep,
};

struct ResolvedProfile {
    name: String,
    revision: SandboxProfileRevision,
}

struct AdmissionAuthority<'a> {
    limit: u16,
    class: SandboxLimitClass,
    lifecycle_lease: Option<&'a str>,
    creation_fence: Option<CreationFence>,
    credential_source: SandboxCredentialSource,
}

impl SandboxService {
    /// Check the same owner credential and service prerequisites as fresh
    /// lifecycle admission without reserving a Sandbox.
    pub async fn check_lifecycle_creation_connections(
        &self,
        caller: &Caller,
        profile: &SandboxProfileContent,
    ) -> Result<(), SandboxServiceError> {
        require(caller, Capability::UseOwnSandboxes)?;
        require(caller, Capability::CreateSandbox)?;
        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(policy_error)?;
        let reaches = profile.credential_services(&policy.baseline_services);
        self.check_creation_connections(caller, profile, &reaches)
            .await
    }

    /// Trusted lifecycle worker admission. Replay precedes current capacity and
    /// dependency checks so a lost response cannot allocate a second Sandbox.
    pub async fn create_for_lifecycle(
        &self,
        caller: &Caller,
        request: CreateSandbox,
    ) -> Result<Sandbox, SandboxServiceError> {
        self.admit_lifecycle(
            caller,
            request,
            None,
            None,
            SandboxCredentialSource::Managed,
        )
        .await
    }

    /// Trusted admission using the initiating lifecycle worker's claim.
    pub async fn create_for_lifecycle_with_lease(
        &self,
        caller: &Caller,
        request: CreateSandbox,
        lease: &str,
        creation_fence: Option<CreationFence>,
    ) -> Result<Sandbox, SandboxServiceError> {
        self.create_for_lifecycle_with_connection(caller, request, lease, creation_fence, None)
            .await
    }

    /// Carry the verified initial connection into the same Sandbox admission.
    pub async fn create_for_lifecycle_with_connection(
        &self,
        caller: &Caller,
        request: CreateSandbox,
        lease: &str,
        creation_fence: Option<CreationFence>,
        connection: Option<InitialServiceConnection>,
    ) -> Result<Sandbox, SandboxServiceError> {
        let source = connection.map_or(SandboxCredentialSource::Managed, |initial| {
            SandboxCredentialSource::ManagedConnection { initial }
        });
        self.admit_lifecycle(caller, request, Some(lease), creation_fence, source)
            .await
    }

    async fn admit_lifecycle(
        &self,
        caller: &Caller,
        request: CreateSandbox,
        lifecycle_lease: Option<&str>,
        creation_fence: Option<CreationFence>,
        credential_source: SandboxCredentialSource,
    ) -> Result<Sandbox, SandboxServiceError> {
        require(caller, Capability::UseOwnSandboxes)?;
        require(caller, Capability::CreateSandbox)?;
        request.origin.validate()?;
        if !matches!(request.origin, SandboxOrigin::Lifecycle { .. }) {
            return Err(SandboxServiceError::InvalidOrigin);
        }
        let exact_name = request.name.clone().map(exact_name).transpose()?;
        if let Some(replay) = self
            .store
            .sandbox_admission_replay(&caller.subject, &request.origin)
            .await
            .map_err(store_error)?
        {
            let sandbox = reconcile_launch_replay(caller, &request, exact_name.as_deref(), replay)?;
            if sandbox.credential.source.initial_connection()
                != credential_source.initial_connection()
            {
                return Err(SandboxServiceError::SandboxOriginConflict);
            }
            return Ok(sandbox);
        }
        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(policy_error)?;
        let profile = self.read_profile(&caller.subject, &request, true).await?;
        self.admit(
            caller,
            request,
            profile,
            AdmissionAuthority {
                limit: policy.max_sandboxes_per_owner,
                class: SandboxLimitClass::Owner,
                lifecycle_lease,
                creation_fence,
                credential_source,
            },
            policy,
        )
        .await
    }

    pub async fn create(
        &self,
        caller: &Caller,
        request: CreateSandbox,
    ) -> Result<Sandbox, SandboxServiceError> {
        self.create_with_fence(caller, request, None).await
    }

    pub async fn create_with_fence(
        &self,
        caller: &Caller,
        request: CreateSandbox,
        creation_fence: Option<CreationFence>,
    ) -> Result<Sandbox, SandboxServiceError> {
        require(caller, Capability::CreateSandbox)?;
        request.origin.validate()?;
        if request.origin != SandboxOrigin::Manual {
            return Err(SandboxServiceError::InvalidOrigin);
        }
        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(policy_error)?;
        let profile = self.read_profile(&caller.subject, &request, false).await?;
        self.admit(
            caller,
            request,
            profile,
            AdmissionAuthority {
                limit: policy.max_sandboxes_per_owner,
                class: SandboxLimitClass::Owner,
                lifecycle_lease: None,
                creation_fence,
                credential_source: SandboxCredentialSource::Managed,
            },
            policy,
        )
        .await
    }

    pub async fn create_for_launch(
        &self,
        caller: &Caller,
        authority: Capability,
        request: CreateSandbox,
    ) -> Result<Sandbox, SandboxServiceError> {
        require_launch(caller, authority)?;
        request.origin.validate()?;
        let SandboxOrigin::Trigger { .. } = &request.origin else {
            return Err(SandboxServiceError::InvalidOrigin);
        };
        let exact_name = request.name.clone().map(exact_name).transpose()?;
        if let Some(replay) = self
            .store
            .sandbox_admission_replay(&caller.subject, &request.origin)
            .await
            .map_err(store_error)?
        {
            return reconcile_launch_replay(caller, &request, exact_name.as_deref(), replay);
        }

        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(policy_error)?;
        let profile = self.read_profile(&caller.subject, &request, true).await?;
        let (limit, limit_class) = match authority {
            Capability::CreateSandbox => (policy.max_sandboxes_per_owner, SandboxLimitClass::Owner),
            Capability::LaunchTriggers => {
                (policy.max_sandboxes_per_visitor, SandboxLimitClass::Visitor)
            }
            _ => unreachable!("launch authority was validated"),
        };
        self.admit(
            caller,
            request,
            profile,
            AdmissionAuthority {
                limit,
                class: limit_class,
                lifecycle_lease: None,
                creation_fence: None,
                credential_source: SandboxCredentialSource::Managed,
            },
            policy,
        )
        .await
    }

    async fn read_profile(
        &self,
        consumer: &SubjectId,
        request: &CreateSandbox,
        frozen: bool,
    ) -> Result<ResolvedProfile, SandboxServiceError> {
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
        if !frozen
            && (profile.retired_at.is_some()
                || (profile.owner != *consumer && profile.visibility != ProfileVisibility::Public))
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
        self.require_trust(
            consumer,
            &profile.owner,
            &revision,
            request.confirmed_digest.as_deref(),
        )
        .await?;
        let name = if frozen
            && profile.owner.as_str() == crate::DELETED_ACCOUNT_SUBJECT
            && profile.visibility == ProfileVisibility::Private
            && profile.retired_at.is_some()
            && profile.name.is_empty()
            && profile.description.is_empty()
        {
            // Retained catalogue metadata has no author-supplied display name.
            // The admission transaction still verifies the frozen Launch pin.
            "Sandbox".into()
        } else {
            profile.name
        };
        Ok(ResolvedProfile { name, revision })
    }

    async fn require_trust(
        &self,
        consumer: &SubjectId,
        owner: &SubjectId,
        revision: &SandboxProfileRevision,
        confirmed_digest: Option<&str>,
    ) -> Result<(), SandboxServiceError> {
        let nested_needs_trust = !foreign_workspace_setups(
            self.workspace_profiles.as_ref(),
            &revision.content.workspace_profiles,
            consumer,
        )
        .await
        .map_err(profile_error)?
        .is_empty();
        let outer_needs_trust = owner != consumer
            && (!revision.content.software_install_commands.is_empty()
                || !revision.content.environment_variables.is_empty());
        if (outer_needs_trust || nested_needs_trust)
            && confirmed_digest != Some(revision.digest.as_str())
        {
            return Err(SandboxServiceError::TrustConfirmationRequired {
                commands_digest: revision.digest.clone(),
            });
        }
        Ok(())
    }

    async fn check_creation_connections(
        &self,
        caller: &Caller,
        profile: &SandboxProfileContent,
        reaches: &[ServiceId],
    ) -> Result<(), SandboxServiceError> {
        self.credentials
            .check_services(
                &caller.subject,
                &profile.connection_services_for_grants(reaches),
            )
            .await
            .map_err(|error| match error {
                super::SandboxCredentialPrerequisiteError::SetupRequired => {
                    SandboxServiceError::CredentialSetupRequired
                }
                super::SandboxCredentialPrerequisiteError::ConnectionsRequired { services } => {
                    SandboxServiceError::ConnectionsRequired { services }
                }
                super::SandboxCredentialPrerequisiteError::Unavailable(detail) => {
                    SandboxServiceError::Unavailable(detail)
                }
            })
    }

    async fn admit(
        &self,
        caller: &Caller,
        request: CreateSandbox,
        profile: ResolvedProfile,
        authority: AdmissionAuthority<'_>,
        policy: SandboxPolicy,
    ) -> Result<Sandbox, SandboxServiceError> {
        let name = requested_name(request.name, &profile.name)?;
        let reaches = profile
            .revision
            .content
            .credential_services(&policy.baseline_services);
        if let Some(initial) = authority.credential_source.initial_connection() {
            // The lifecycle worker checked this specific authorization. The
            // generic prerequisite would resolve the service by slug instead.
            if !initial.selection.matches_scope(&reaches) {
                return Err(SandboxServiceError::InvalidServiceAccess);
            }
            let remaining: Vec<_> = reaches
                .iter()
                .filter(|service| *service != &initial.selection.service)
                .cloned()
                .collect();
            self.check_creation_connections(caller, &profile.revision.content, &remaining)
                .await?;
        } else {
            self.check_creation_connections(caller, &profile.revision.content, &reaches)
                .await?;
        }
        let now = now_rfc3339();
        let sign_in = match profile.revision.content.model_access {
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
            profile: SandboxProfileSnapshot::from(&profile.revision),
            status: SandboxStatus::Provisioning,
            failure: None,
            runtime: None,
            credential: SandboxCredentialState {
                source: authority.credential_source,
                service_grants: Vec::new(),
                service_scope: None,
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
                max_workspaces: profile.revision.content.max_workspaces,
                max_agents_per_workspace: profile.revision.content.max_agents_per_workspace,
            },
            lifecycle: Default::default(),
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
        let mut admission =
            SandboxAdmission::new(sandbox, name, authority.limit, now).map_err(store_error)?;
        admission.limit_class = authority.class;
        admission.creation_fence = authority.creation_fence;
        if let Some(token) = authority.lifecycle_lease {
            admission = admission.with_lifecycle_lease(token);
        }
        let result = self
            .store
            .admit_catalogue_sandbox(admission)
            .await
            .map_err(store_error)?;
        match result {
            SandboxAdmissionResult::Existing(sandbox) => Ok(sandbox),
            SandboxAdmissionResult::Created(sandbox) => {
                let locator = MarketplaceLocator::Sandbox(sandbox.profile.id.clone());
                if let Err(error) = self.profile_uses.record_profile_use(&locator).await {
                    tracing::warn!(
                        sandbox_id = %sandbox.id,
                        profile_id = locator.as_str(),
                        %error,
                        "sandbox profile use was not recorded"
                    );
                }
                Ok(sandbox)
            }
        }
    }
}

fn reconcile_launch_replay(
    caller: &Caller,
    request: &CreateSandbox,
    exact_name: Option<&str>,
    replay: SandboxAdmissionReplay,
) -> Result<Sandbox, SandboxServiceError> {
    replay.validate().map_err(store_error)?;
    let name_matches = match (exact_name, &replay.name) {
        (Some(requested), SandboxAdmissionName::Exact(stored)) => requested == stored,
        (None, SandboxAdmissionName::Allocate(_)) => true,
        _ => false,
    };
    if replay.sandbox.owner != caller.subject
        || replay.sandbox.origin != request.origin
        || replay.sandbox.profile.id != request.profile.id
        || replay.sandbox.profile.revision != request.profile.revision
        || !name_matches
    {
        return Err(SandboxServiceError::SandboxOriginConflict);
    }
    Ok(replay.sandbox)
}

fn require_launch(caller: &Caller, authority: Capability) -> Result<(), SandboxServiceError> {
    (matches!(
        authority,
        Capability::LaunchTriggers | Capability::CreateSandbox
    ) && caller.authorization.allows(authority))
    .then_some(())
    .ok_or(SandboxServiceError::Forbidden)
}
