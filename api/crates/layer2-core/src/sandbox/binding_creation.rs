use super::binding::{BindingAdmissionFence, ProviderBinding};
use super::service::{policy_error, store_error};
use super::store::{SandboxAdmission, SandboxAdmissionName, SandboxAdmissionResult};
use super::*;
use crate::profiles::{ModelAccess, SandboxProfileRevision, SandboxProfileSnapshot};

impl SandboxService {
    pub(crate) async fn create_for_binding(
        &self,
        binding: &ProviderBinding,
        revision: &SandboxProfileRevision,
    ) -> Result<Sandbox, SandboxServiceError> {
        let content = &revision.content;
        let at = crate::now_rfc3339();
        let candidate = Sandbox {
            id: binding.sandbox_id.clone(),
            owner: binding.owner.clone(),
            name: format!(
                "binding-{}",
                binding
                    .sandbox_id
                    .as_str()
                    .chars()
                    .take(48)
                    .collect::<String>()
            ),
            origin: SandboxOrigin::Manual,
            profile: SandboxProfileSnapshot::from(revision),
            status: SandboxStatus::Provisioning,
            failure: None,
            runtime: None,
            credential: SandboxCredentialState {
                source: binding.credential_source(),
                service_grants: vec![],
                service_scope: None,
                reaches: content.nyxid_services.clone(),
                key_generation: 0,
                delivered: false,
                current: None,
                pending: None,
                revoke_debt: vec![],
                sign_in: match content.model_access {
                    ModelAccess::ChronoaiManaged => SandboxSignInState::NotRequired,
                    ModelAccess::OwnCredentials => SandboxSignInState::Required,
                },
            },
            work: SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, at.clone()),
            limits: SandboxLimits {
                max_workspaces: content.max_workspaces,
                max_agents_per_workspace: content.max_agents_per_workspace,
            },
            lifecycle: Default::default(),
            last_activity_at: at.clone(),
            created_at: at.clone(),
            updated_at: at.clone(),
            retired_at: None,
            purge_after: None,
            revision: 1,
        };
        let fence = BindingAdmissionFence::environment(binding);
        fence
            .require_sandbox(binding, &candidate)
            .map_err(store_error)?;
        if let Some(existing) = self
            .store
            .get_sandbox(&candidate.id)
            .await
            .map_err(store_error)?
        {
            fence
                .require_sandbox(binding, &existing)
                .map_err(store_error)?;
            if existing.profile != candidate.profile {
                return Err(SandboxServiceError::Corrupt(
                    "binding profile identity changed".into(),
                ));
            }
            return Ok(existing);
        }
        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(policy_error)?;
        let name = SandboxAdmissionName::Exact(candidate.name.clone());
        let mut admission =
            SandboxAdmission::new(candidate, name, policy.max_sandboxes_per_owner, at)
                .map_err(store_error)?;
        admission.binding = Some(fence);
        match self
            .store
            .admit_sandbox(admission)
            .await
            .map_err(store_error)?
        {
            SandboxAdmissionResult::Created(value) | SandboxAdmissionResult::Existing(value) => {
                Ok(value)
            }
        }
    }
}

impl SandboxService {
    pub(crate) async fn retire_for_binding(
        &self,
        binding: &ProviderBinding,
        id: &crate::SandboxId,
    ) -> Result<bool, SandboxServiceError> {
        use super::binding::{BindingState, BindingWork};
        binding.validate().map_err(SandboxServiceError::Corrupt)?;
        let allowed = binding.existing_agent.is_none()
            && ((binding.state == BindingState::Retired
                && matches!(
                    binding.work,
                    BindingWork::RetireCurrentConversations | BindingWork::RetireCurrentSandbox
                )
                && *id == binding.sandbox_id)
                || (matches!(
                    binding.work,
                    BindingWork::RetirePreviousConversations | BindingWork::RetirePreviousSandbox
                ) && binding
                    .previous
                    .as_ref()
                    .is_some_and(|old| old.retire_sandbox && old.sandbox_id == *id)));
        if !allowed {
            return Err(SandboxServiceError::InvalidState);
        }
        let Some(mut sandbox) = self.store.get_sandbox(id).await.map_err(store_error)? else {
            return Ok(true);
        };
        if sandbox.owner != binding.owner
            || sandbox.credential.source != binding.credential_source()
        {
            return Err(SandboxServiceError::Forbidden);
        }
        if sandbox.status == SandboxStatus::Retired {
            return Ok(true);
        }
        if sandbox.status == SandboxStatus::Retiring {
            return Ok(false);
        }
        let previous = sandbox.status;
        let expected = sandbox.revision;
        let at = crate::now_rfc3339();
        sandbox
            .begin_retirement(at.clone())
            .map_err(|_| SandboxServiceError::InvalidState)?;
        self.store
            .commit(super::store::SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: expected,
                sandbox: Box::new(sandbox),
                event_at: at,
                event: SandboxEventKind::StatusChanged {
                    from: previous,
                    to: SandboxStatus::Retiring,
                },
            })
            .await
            .map_err(store_error)?;
        Ok(false)
    }
}
