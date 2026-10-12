use crate::profiles::{
    MarketplaceLocator, ModelAccess, ProfileVisibility, SandboxProfileRevision,
    SandboxProfileSnapshot,
};
use crate::{now_rfc3339, Caller, Capability, SandboxId, SubjectId};

use super::service::{
    policy_error, profile_error, require, store_error, CreateSandbox, SandboxPolicy,
    SandboxService, SandboxServiceError,
};
use super::service_admission::{exact_name, requested_name};
use super::service_trust::{
    foreign_workspace_setups, validate_sandbox_profile, validate_sandbox_revision,
};
use super::store::{
    SandboxAdmission, SandboxAdmissionName, SandboxAdmissionReplay, SandboxAdmissionResult,
};
use super::{
    Sandbox, SandboxCredentialState, SandboxLimits, SandboxOrigin, SandboxSignInState,
    SandboxStatus, SandboxWorkState, SandboxWorkStep,
};

struct ResolvedProfile {
    name: String,
    revision: SandboxProfileRevision,
}

impl SandboxService {
    /// Trusted lifecycle worker admission. Replay precedes current capacity and
    /// dependency checks so a lost response cannot allocate a second Sandbox.
    pub async fn create_for_lifecycle(
        &self,
        caller: &Caller,
        request: CreateSandbox,
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
            return reconcile_launch_replay(caller, &request, exact_name.as_deref(), replay);
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
            policy.max_sandboxes_per_owner,
            policy,
        )
        .await
    }

    pub async fn create(
        &self,
        caller: &Caller,
        request: CreateSandbox,
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
            policy.max_sandboxes_per_owner,
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
        let limit = match authority {
            Capability::CreateSandbox => policy.max_sandboxes_per_owner,
            Capability::LaunchTriggers => policy.max_sandboxes_per_visitor,
            _ => unreachable!("launch authority was validated"),
        };
        self.admit(caller, request, profile, limit, policy).await
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
        Ok(ResolvedProfile {
            name: profile.name,
            revision,
        })
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

    async fn admit(
        &self,
        caller: &Caller,
        request: CreateSandbox,
        profile: ResolvedProfile,
        limit: u16,
        policy: SandboxPolicy,
    ) -> Result<Sandbox, SandboxServiceError> {
        let name = requested_name(request.name, &profile.name)?;
        let reaches = profile
            .revision
            .content
            .credential_services(&policy.baseline_services);
        self.credentials
            .check_services(
                &caller.subject,
                &profile
                    .revision
                    .content
                    .connection_services_for_grants(&reaches),
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
            })?;
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
                source: Default::default(),
                service_grants: Vec::new(),
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
        let result = self
            .store
            .admit_sandbox(SandboxAdmission::new(sandbox, name, limit, now).map_err(store_error)?)
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
