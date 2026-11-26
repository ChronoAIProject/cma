//! Verify a fresh, owner-scoped repository snapshot before paid allocation.

use crate::store::{PublicRepositoryPin, RepositoryAccessPin};
use crate::trigger::{GitHubRepositoryCheck, GitHubScopeCheck, GITHUB_SERVICE};
use layer1_public::rest::{
    AgentLifecycleRepository, GitHubRepositoryNeed, RepositoryAccessRefusal, RepositoryScopeRepair,
};
use layer2_core::profiles::authoring::AuthoringSourceError;
use layer2_core::sandbox::{
    InitialServiceConnection, RepositoryCredentialRequirement, ServiceConnectionSelection,
};

use super::*;

mod public;
#[cfg(test)]
pub(super) use public::PublicRepositoryReadiness;

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
    pub(super) fn with_managed_repository_preflight(self: Arc<Self>, enabled: bool) -> Arc<Self> {
        self.managed_repository_preflight
            .set(enabled)
            .expect("repository preflight is configured once before use");
        self
    }

    pub(crate) fn with_repository_preflight_config(
        self: Arc<Self>,
        config: &crate::config::Config,
        broker: Arc<crate::trigger::CmaServiceConnectionBroker>,
    ) -> Arc<Self> {
        let lifecycle =
            self.with_managed_repository_preflight(config.features.github_repository_preflight);
        assert!(lifecycle.managed_repository_broker.set(broker).is_ok());
        lifecycle
            .managed_repository_runtime_images
            .set((
                config.runtime.sandbox_image.clone(),
                config.runtime.github_repository_runtime_images.clone(),
            ))
            .expect("repository runtime qualification is configured once before use");
        lifecycle
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
        retained_public: Option<&PublicRepositoryPin>,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        if !input.behavior.uses_repository_profile()
            || input.request.sandbox_id.is_some()
            || input.request.workspace_id.is_some()
        {
            return Ok(());
        }
        let Some(repository) = input.request.repository.as_ref() else {
            return Ok(());
        };
        let selected_authority =
            repository.operation.is_some() || repository.connection_id.is_some();
        if matches!(
            input.behavior,
            LifecycleBehavior::RepositoryChoice | LifecycleBehavior::PublicRepository
        ) {
            if let Some(pin) = retained_public {
                if input.repository_access_pin.is_some()
                    || retained_commit.is_some()
                    || selected_authority
                {
                    return Err(AgentLifecycleProvisioningError::Conflict(
                        "Public source preparation cannot carry selected connection authority."
                            .into(),
                    ));
                }
                return self.preflight_public_repository(caller, input, pin).await;
            }
            if input.repository_access_pin.is_none()
                && retained_commit.is_none()
                && !selected_authority
                && self.try_public_repository(input).await?
            {
                return Ok(());
            }
            if input.behavior == LifecycleBehavior::PublicRepository {
                return self
                    .wait_for_managed_repository_access(
                        caller,
                        input,
                        RepositoryAccessRefusal::VerificationUnavailable,
                        None,
                    )
                    .await;
            }
            // No anonymous proof was established. Use the existing selected
            // connection proof or typed repair, keyed to the original operation.
            input.behavior = LifecycleBehavior::ManagedRepository;
        } else if retained_public.is_some() {
            return Err(AgentLifecycleProvisioningError::Conflict(
                "The retained public source does not match the operation mode.".into(),
            ));
        }
        let requires_managed =
            input.behavior == LifecycleBehavior::ManagedRepository || selected_authority;
        if input.repository_access_pin.is_some() || requires_managed {
            if input.repository_access_pin.is_none()
                && self.managed_repository_preflight.get() != Some(&true)
            {
                return self
                    .wait_for_managed_repository_access(
                        caller,
                        input,
                        RepositoryAccessRefusal::VerificationUnavailable,
                        None,
                    )
                    .await;
            }
            return self
                .preflight_managed_repository(caller, input, retained_commit)
                .await;
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
                let repository = input
                    .request
                    .repository
                    .as_ref()
                    .expect("repository checked above");
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

    async fn preflight_managed_repository(
        &self,
        caller: &Caller,
        input: &mut ProvisionInput,
        retained_commit: Option<&str>,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        use RepositoryAccessRefusal as Refusal;
        let repository = input
            .request
            .repository
            .as_ref()
            .expect("repository checked by caller");
        let need = repository
            .operation
            .unwrap_or(GitHubRepositoryNeed::PrivateRepositoryRead);
        let requirement = match need {
            GitHubRepositoryNeed::PublicRepositoryWrite => {
                RepositoryCredentialRequirement::PublicRepositoryWrite
            }
            GitHubRepositoryNeed::PrivateRepositoryRead => {
                RepositoryCredentialRequirement::PrivateRepositoryRead
            }
        };
        let pin = input.repository_access_pin.as_ref();
        let selected = pin.map(|pin| pin.connection.selection.connection_id.as_str());
        if let Some(pin) = pin {
            if pin.owner != caller.subject.as_str()
                || pin.repository != repository.url
                || pin.connection.requirement != requirement
                || pin.connection.selection.service.as_str() != GITHUB_SERVICE
                || repository
                    .connection_id
                    .as_deref()
                    .is_some_and(|id| Some(id) != selected)
                || Some(pin.commit.as_str()) != retained_commit
            {
                return Err(AgentLifecycleProvisioningError::Conflict(
                    "The retained GitHub repository authorization does not match this request."
                        .into(),
                ));
            }
        }

        let Some((default, qualified)) = self.managed_repository_runtime_images.get() else {
            return self
                .wait_for_managed_repository_access(
                    caller,
                    input,
                    Refusal::VerificationUnavailable,
                    None,
                )
                .await;
        };
        let image = {
            let image = pin
                .map(|pin| &pin.runtime_image)
                .or(input.runtime_image.as_ref())
                .unwrap_or(default);
            (input
                .runtime_image
                .as_ref()
                .is_none_or(|resolved| resolved == image)
                && qualified.contains(image))
            .then_some(image)
        };
        let Some(image) = image else {
            return self
                .wait_for_managed_repository_access(
                    caller,
                    input,
                    Refusal::RuntimeUnsupported,
                    None,
                )
                .await;
        };

        let Some(broker) = self.managed_repository_broker.get() else {
            return self
                .wait_for_managed_repository_access(
                    caller,
                    input,
                    Refusal::VerificationUnavailable,
                    None,
                )
                .await;
        };
        let check = broker
            .verify_github_repository(&caller.subject, repository, selected, retained_commit)
            .await;
        let (reason, repair) = match check {
            GitHubRepositoryCheck::Verified(proved) => {
                let commit = valid_commit(&proved.commit)?;
                let image = image.clone();
                if pin.is_none() {
                    let pin = RepositoryAccessPin {
                        owner: caller.subject.to_string(),
                        repository: repository.url.clone(),
                        commit: commit.clone(),
                        runtime_image: image.clone(),
                        connection: InitialServiceConnection {
                            selection: ServiceConnectionSelection {
                                service: layer2_core::ServiceId::new(GITHUB_SERVICE),
                                connection_id: proved.connection_id,
                            },
                            requirement,
                            repository: proved.repository,
                        },
                    };
                    self.operations
                        .pin_repository_access(&input.lease, &pin)
                        .await
                        .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
                    input.repository_access_pin = Some(pin);
                }
                input.runtime_image = Some(image);
                let repository = input
                    .request
                    .repository
                    .as_mut()
                    .expect("verified repository exists");
                repository.reference = Some(commit);
                repository.operation = Some(need);
                let binding = &input
                    .repository_access_pin
                    .as_ref()
                    .expect("repository access pinned")
                    .connection;
                repository.url = format!("https://github.com/{}", binding.repository);
                repository.connection_id = Some(binding.selection.connection_id.clone());
                if input.request.initial_message.is_none() {
                    let mut setup = format!(
                        "CMA prepared the selected GitHub source at commit {}. Confirm the current workspace and report readiness. Preserve all files and local changes. Use gh api with the path in CMA_GITHUB_ACCESS_PATH to read the selected connection's repository access and repair details.",
                        repository.reference.as_deref().expect("commit assigned")
                    );
                    if input.request.desktop {
                        setup.push_str(" The owner can use the shared desktop for later tasks.");
                    }
                    input.first_message = Some(setup);
                }
                return Ok(());
            }
            GitHubRepositoryCheck::RepositoryUnavailable => (Refusal::RepositoryUnavailable, None),
            GitHubRepositoryCheck::ReferenceUnavailable => (Refusal::ReferenceUnavailable, None),
            GitHubRepositoryCheck::Scope(GitHubScopeCheck::MissingConnection) => {
                (Refusal::MissingConnection, None)
            }
            GitHubRepositoryCheck::Scope(GitHubScopeCheck::InsufficientScope {
                connection_id,
                required_scope,
                manage_url,
            }) => (
                Refusal::InsufficientScope,
                Some(RepositoryScopeRepair {
                    connection_id,
                    operation: need,
                    required_scope: required_scope.into(),
                    manage_url,
                }),
            ),
            GitHubRepositoryCheck::Scope(_) => (Refusal::VerificationUnavailable, None),
        };
        self.wait_for_managed_repository_access(caller, input, reason, repair)
            .await
    }

    async fn wait_for_managed_repository_access(
        &self,
        caller: &Caller,
        input: &ProvisionInput,
        reason: RepositoryAccessRefusal,
        repair: Option<RepositoryScopeRepair>,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        let mut content = profiles::inline_sandbox_content(
            &input.request,
            input.runtime_image.clone(),
            input.capacity,
            input.limits,
            input.behavior,
            None,
        )?;
        let github = layer2_core::ServiceId::new(GITHUB_SERVICE);
        if !content.nyxid_services.contains(&github) {
            content.nyxid_services.push(github);
        }
        let profile = self
            .repository_connection_profile(caller, input, content)
            .await?;
        Err(AgentLifecycleProvisioningError::RepositoryAccessRequired {
            connection_target: admission::connection_profile_target(&profile),
            reason,
            repair: repair.map(Box::new),
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
