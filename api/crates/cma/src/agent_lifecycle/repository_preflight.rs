//! Verify a fresh, owner-scoped repository snapshot before paid allocation.

use layer1_public::rest::AgentLifecycleRepository;
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
            || input.request.initial_message.is_none()
        {
            return Ok(());
        }
        let Some(repository) = input.request.repository.as_ref() else {
            return Ok(());
        };
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
        Err(admission::sandbox_admission_error(
            error,
            &SandboxProfileRef {
                id: SandboxProfileId(target.profile_id.to_string()),
                revision: target.revision,
            },
        ))
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
