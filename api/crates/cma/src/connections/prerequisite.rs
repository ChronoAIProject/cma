use std::{collections::BTreeSet, sync::Arc};

use layer2_core::{
    sandbox::{SandboxCredentialPrerequisite, SandboxCredentialPrerequisiteError},
    trigger_domain::ServiceConnectionBroker,
    ServiceId, SubjectId,
};

pub(crate) struct ConnectionPrerequisite {
    identity: Arc<dyn SandboxCredentialPrerequisite>,
    connections: Arc<dyn ServiceConnectionBroker>,
    policy: Arc<dyn layer2_core::sandbox::SandboxPolicySource>,
    exact_github: Option<Arc<crate::trigger::CmaServiceConnectionBroker>>,
}

impl ConnectionPrerequisite {
    pub(crate) fn with_exact_github(
        mut self: Arc<Self>,
        broker: Arc<crate::trigger::CmaServiceConnectionBroker>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("connection prerequisite is configured before sharing")
            .exact_github = Some(broker);
        self
    }

    pub(crate) fn new(
        identity: Arc<dyn SandboxCredentialPrerequisite>,
        connections: Arc<dyn ServiceConnectionBroker>,
        policy: Arc<dyn layer2_core::sandbox::SandboxPolicySource>,
    ) -> Arc<Self> {
        Arc::new(Self {
            identity,
            connections,
            policy,
            exact_github: None,
        })
    }
}

#[async_trait::async_trait]
impl SandboxCredentialPrerequisite for ConnectionPrerequisite {
    async fn check_sandbox(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
    ) -> Result<(), SandboxCredentialPrerequisiteError> {
        if !sandbox.credential.source.is_managed() {
            return if sandbox.credential.validate() && sandbox.credential.current.is_some() {
                Ok(())
            } else {
                Err(SandboxCredentialPrerequisiteError::SetupRequired)
            };
        }
        let reaches = sandbox.credential.effective_reaches();
        let mut services = sandbox
            .profile
            .content
            .connection_services_for_grants(&reaches);
        if let Some(initial) = sandbox.credential.selected_repository_connection() {
            use crate::trigger::{GitHubRepositoryNeed, GitHubScopeCheck};
            use layer2_core::sandbox::RepositoryCredentialRequirement;
            let broker = self.exact_github.as_ref().ok_or_else(unavailable)?;
            let need = match initial.requirement {
                RepositoryCredentialRequirement::PublicRepositoryWrite => {
                    GitHubRepositoryNeed::PublicRepositoryWrite
                }
                RepositoryCredentialRequirement::PrivateRepositoryRead => {
                    GitHubRepositoryNeed::PrivateRepositoryRead
                }
            };
            match broker
                .exact_github_scope_check(&sandbox.owner, &initial.selection.connection_id, need)
                .await
            {
                GitHubScopeCheck::ScopeSatisfied { .. } => (),
                GitHubScopeCheck::InsufficientScope { .. }
                | GitHubScopeCheck::MissingConnection => {
                    return Err(SandboxCredentialPrerequisiteError::ConnectionsRequired {
                        services: vec![initial.selection.service.clone()],
                    });
                }
                GitHubScopeCheck::VerificationUnavailable => return Err(unavailable()),
            }
            services.retain(|service| service != &initial.selection.service);
        }
        self.check_services(&sandbox.owner, &services).await
    }

    async fn connection_services(
        &self,
        profile: &layer2_core::profiles::SandboxProfileContent,
    ) -> Result<Vec<ServiceId>, SandboxCredentialPrerequisiteError> {
        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(|_| unavailable())?;
        Ok(profile.connection_services_for_grants(
            &profile.credential_services(&policy.baseline_services),
        ))
    }

    async fn check(&self, owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        self.identity.check(owner).await
    }

    async fn check_services(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<(), SandboxCredentialPrerequisiteError> {
        self.check(owner).await.inspect_err(|error| {
            let error_kind = match error {
                SandboxCredentialPrerequisiteError::SetupRequired => "credential_setup_required",
                SandboxCredentialPrerequisiteError::ConnectionsRequired { .. } => {
                    "connections_required"
                }
                SandboxCredentialPrerequisiteError::Unavailable(_) => "identity_unavailable",
            };
            tracing::warn!(boundary = "owner_credentials", error_kind,
                owner_id = %owner.as_str().chars().take(128).collect::<String>(),
                "Sandbox owner credential prerequisite failed");
        })?;
        let requirements = self
            .connections
            .requirements(owner, services)
            .await
            .map_err(|_| {
                tracing::warn!(boundary = "service_connections", error_kind = "connection_lookup_unavailable",
                    owner_id = %owner.as_str().chars().take(128).collect::<String>(),
                    "Sandbox connection prerequisite failed");
                unavailable()
            })?;
        let expected = services.iter().collect::<BTreeSet<_>>();
        let actual = requirements
            .iter()
            .map(|requirement| &requirement.service)
            .collect::<BTreeSet<_>>();
        if requirements.len() != expected.len() || expected != actual {
            tracing::warn!(boundary = "service_connections", error_kind = "connection_set_mismatch",
                owner_id = %owner.as_str().chars().take(128).collect::<String>(),
                "Sandbox connection prerequisite failed");
            return Err(unavailable());
        }
        let missing = requirements
            .into_iter()
            .filter(|requirement| requirement.needs_connection())
            .map(|requirement| requirement.service)
            .collect::<Vec<_>>();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(SandboxCredentialPrerequisiteError::ConnectionsRequired { services: missing })
        }
    }
}

fn unavailable() -> SandboxCredentialPrerequisiteError {
    SandboxCredentialPrerequisiteError::Unavailable(
        "required connections could not be verified".into(),
    )
}
