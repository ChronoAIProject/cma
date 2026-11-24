//! Verify a fresh, owner-scoped repository snapshot before paid allocation.

use layer1_public::rest::{AgentLifecycleRepository, RepositoryAccessRefusal};
use layer2_core::profiles::authoring::AuthoringSourceError;

use super::*;

#[async_trait]
pub(super) trait RepositoryReadiness: Send + Sync {
    /// Resolve the exact repository with the owner and service credential mode
    /// that the runtime will use. A public metadata lookup cannot supply this proof.
    async fn resolve_for_runtime(
        &self,
        owner: &SubjectId,
        repository: &AgentLifecycleRepository,
    ) -> Result<String, AuthoringSourceError>;
}

impl InlineAgentLifecycle {
    pub(crate) fn with_managed_repository_preflight(self: Arc<Self>, enabled: bool) -> Arc<Self> {
        self.managed_repository_preflight
            .set(enabled)
            .expect("repository preflight is configured once before use");
        self
    }

    #[cfg(test)]
    pub(in crate::agent_lifecycle) fn with_repository_preflight(
        self: Arc<Self>,
        probe: Arc<dyn RepositoryReadiness>,
    ) -> Arc<Self> {
        assert!(self.repository_preflight.set(probe).is_ok());
        self
    }

    pub(super) async fn preflight_repository(
        &self,
        caller: &Caller,
        input: &mut ProvisionInput,
        retained_commit: Option<&str>,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        if input.behavior != LifecycleBehavior::PromptRepository
            || input.request.sandbox_id.is_some()
            || input.request.workspace_id.is_some()
        {
            return Ok(());
        }
        let Some(repository) = input.request.repository.as_ref() else {
            return Ok(());
        };
        if self.managed_repository_preflight.get() == Some(&true) {
            return self.wait_for_managed_repository_access(caller, input).await;
        }
        if input.request.initial_message.is_none() {
            return Ok(());
        }
        let commit = match retained_commit {
            Some(commit) => valid_commit(commit)?,
            None if input.existing_sandbox_id.is_some() => return Ok(()),
            None => {
                let Some(probe) = self.repository_preflight.get() else {
                    return Ok(());
                };
                self.require_repository_connections(caller, input).await?;
                let commit = resolve(probe.as_ref(), &caller.subject, repository).await?;
                self.operations
                    .pin_repository_commit(&input.lease, &commit)
                    .await
                    .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
                tracing::debug!(
                    "Repository admission pinned the verified commit before allocation"
                );
                commit
            }
        };
        if let Some(repository) = &mut input.request.repository {
            repository.reference = Some(commit);
        }
        Ok(())
    }

    async fn wait_for_managed_repository_access(
        &self,
        caller: &Caller,
        input: &ProvisionInput,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        // Both the runtime archive reader and Agent setup use the credentialed
        // slug. Public metadata can become stale before dispatch, and a retained
        // commit proves only content identity. Hold until runtime dispatch can
        // prove the effective grant or enforce anonymous source access.
        tracing::info!(operation_id = %input.lease.id, "Managed repository access verification paused");
        let mut content = profiles::inline_sandbox_content(
            &input.request,
            input.runtime_image.clone(),
            input.capacity,
            input.limits,
            input.behavior,
            None,
        )?;
        let github = layer2_core::ServiceId::new(crate::trigger::GITHUB_SERVICE);
        if !content.nyxid_services.contains(&github) {
            content.nyxid_services.push(github);
        }
        let profile = self
            .repository_connection_profile(caller, input, content)
            .await?;
        Err(AgentLifecycleProvisioningError::RepositoryAccessRequired {
            connection_target: admission::connection_profile_target(&profile),
            reason: RepositoryAccessRefusal::VerificationUnavailable,
        })
    }

    async fn require_repository_connections(
        &self,
        caller: &Caller,
        input: &ProvisionInput,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        let content = profiles::inline_sandbox_content(
            &input.request,
            input.runtime_image.clone(),
            input.capacity,
            input.limits,
            input.behavior,
            None,
        )?;
        let error = match self
            .sandbox
            .check_lifecycle_creation_connections(caller, &content)
            .await
        {
            Ok(()) => return Ok(()),
            Err(
                error @ (SandboxServiceError::ConnectionsRequired { .. }
                | SandboxServiceError::CredentialSetupRequired),
            ) => error,
            Err(error) => return Err(sandbox_error(error)),
        };
        // The wizard needs an owner-authorized profile with these exact services.
        // Leave the real Workspace and Sandbox profile keys free for the pinned ref.
        let profile = self
            .repository_connection_profile(caller, input, content)
            .await?;
        Err(admission::sandbox_admission_error(error, &profile))
    }

    async fn repository_connection_profile(
        &self,
        caller: &Caller,
        input: &ProvisionInput,
        content: SandboxProfileContent,
    ) -> Result<SandboxProfileRef, AgentLifecycleProvisioningError> {
        let target = self
            .profiles
            .create_sandbox_configured_for_owner(
                &caller.subject,
                &format!("agent-manager:{}:repository-connection", input.lease.id),
                layer2_core::profiles::PrivateSandboxProfileInput {
                    name: profiles::display_name(&input.request.name, " connection"),
                    description: "Inline Agent Manager repository connection".into(),
                    content,
                    availability: input.sandbox_availability,
                },
            )
            .await
            .map_err(validation::profile_error)?;
        Ok(SandboxProfileRef {
            id: SandboxProfileId(target.profile_id.to_string()),
            revision: target.revision,
        })
    }
}

async fn resolve(
    probe: &dyn RepositoryReadiness,
    owner: &SubjectId,
    repository: &AgentLifecycleRepository,
) -> Result<String, AgentLifecycleProvisioningError> {
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        probe.resolve_for_runtime(owner, repository),
    )
    .await
    .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
    match result {
        Ok(commit) => valid_commit(&commit),
        Err(AuthoringSourceError::RepositoryNotFound) => {
            Err(AgentLifecycleProvisioningError::Conflict(
                "repository_unavailable: The repository could not be read by this connection."
                    .into(),
            ))
        }
        Err(AuthoringSourceError::ReferenceNotFound) => {
            Err(AgentLifecycleProvisioningError::Conflict(
                "repository_ref_not_found: The requested repository ref could not be read.".into(),
            ))
        }
        Err(_) => Err(AgentLifecycleProvisioningError::Unavailable),
    }
}

fn valid_commit(commit: &str) -> Result<String, AgentLifecycleProvisioningError> {
    if matches!(commit.len(), 40 | 64) && commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(commit.to_ascii_lowercase());
    }
    Err(AgentLifecycleProvisioningError::Unavailable)
}
