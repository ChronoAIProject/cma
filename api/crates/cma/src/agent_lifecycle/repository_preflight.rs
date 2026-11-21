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
        owner: &SubjectId,
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
        let Some(repository) = input.request.repository.as_mut() else {
            return Ok(());
        };
        if let Some(commit) = retained_commit {
            repository.reference = Some(valid_commit(commit)?);
            return Ok(());
        }
        if input.existing_sandbox_id.is_some() {
            return Ok(());
        }
        let Some(probe) = self.repository_preflight.get() else {
            // No verified authority can perform this check in the live composition yet.
            return Ok(());
        };
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            probe.resolve_for_runtime(owner, repository),
        )
        .await
        .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        let commit = match result {
            Ok(commit) => valid_commit(&commit)?,
            Err(AuthoringSourceError::RepositoryNotFound) => {
                return Err(AgentLifecycleProvisioningError::Conflict(
                    "repository_unavailable: The repository could not be read by this connection."
                        .into(),
                ))
            }
            Err(AuthoringSourceError::ReferenceNotFound) => {
                return Err(AgentLifecycleProvisioningError::Conflict(
                    "repository_ref_not_found: The requested repository ref could not be read."
                        .into(),
                ))
            }
            Err(_) => return Err(AgentLifecycleProvisioningError::Unavailable),
        };
        self.operations
            .pin_repository_commit(&input.lease, &commit)
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        repository.reference = Some(commit);
        tracing::debug!("Repository admission pinned the verified commit before allocation");
        Ok(())
    }
}

fn valid_commit(commit: &str) -> Result<String, AgentLifecycleProvisioningError> {
    if matches!(commit.len(), 40 | 64) && commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(commit.to_ascii_lowercase());
    }
    Err(AgentLifecycleProvisioningError::Unavailable)
}
