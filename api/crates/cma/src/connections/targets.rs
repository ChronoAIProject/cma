use std::sync::Arc;

use layer2_core::{
    profiles::{ProfileVisibility, SandboxProfileRef, SandboxProfileStore},
    sandbox::{
        store::{AgentStore, SandboxStore, WorkspaceStore},
        SandboxPolicySource, SandboxStatus,
    },
    trigger_domain::{LaunchStore, TriggerProfileReader, TriggerStore},
    AgentId, Caller, Capability, LaunchId, SandboxId, ServiceId, WorkspaceId,
};
use serde::{Deserialize, Serialize};

use crate::store::MongoStores;

use super::browser::ConnectionError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ConnectionTarget {
    SandboxProfile { profile: SandboxProfileRef },
    Sandbox { id: SandboxId },
    Workspace { id: WorkspaceId },
    Agent { id: AgentId },
    AgentAccessRequest { id: String },
    AgentServiceAccess { id: AgentId, services: Vec<String> },
    Launch { id: LaunchId },
}

#[async_trait::async_trait]
pub(super) trait ConnectionTargets: Send + Sync {
    async fn services(
        &self,
        caller: &Caller,
        target: &ConnectionTarget,
    ) -> Result<Vec<ServiceId>, ConnectionError>;
}

pub(super) struct StoredConnectionTargets {
    pub stores: Arc<MongoStores>,
    pub policy: Arc<dyn SandboxPolicySource>,
}

impl StoredConnectionTargets {
    async fn sandbox(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<Vec<ServiceId>, ConnectionError> {
        let sandbox = self
            .stores
            .sandbox_model
            .get_sandbox(id)
            .await
            .map_err(|_| ConnectionError::Unavailable)?
            .filter(|sandbox| {
                sandbox.owner == caller.subject && sandbox.status != SandboxStatus::Retired
            })
            .ok_or(ConnectionError::NotFound)?;
        Ok(sandbox
            .profile
            .content
            .connection_services_for_grants(&sandbox.credential.effective_reaches()))
    }

    async fn profile_services(
        &self,
        content: &layer2_core::profiles::SandboxProfileContent,
    ) -> Result<Vec<ServiceId>, ConnectionError> {
        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(|_| ConnectionError::Unavailable)?;
        Ok(content.connection_services_for_grants(
            &content.credential_services(&policy.baseline_services),
        ))
    }
}

#[async_trait::async_trait]
impl ConnectionTargets for StoredConnectionTargets {
    async fn services(
        &self,
        caller: &Caller,
        target: &ConnectionTarget,
    ) -> Result<Vec<ServiceId>, ConnectionError> {
        if ![
            Capability::CreateSandbox,
            Capability::UseOwnSandboxes,
            Capability::LaunchTriggers,
        ]
        .iter()
        .any(|capability| caller.authorization.allows(*capability))
        {
            return Err(ConnectionError::Forbidden);
        }
        match target {
            ConnectionTarget::AgentServiceAccess { id, services } => {
                scope::services(
                    &self.stores.sandbox_model,
                    self.policy.as_ref(),
                    caller,
                    id,
                    services,
                )
                .await
            }
            ConnectionTarget::AgentAccessRequest { id } => {
                if !caller.authorization.allows(Capability::UseOwnSandboxes) {
                    return Err(ConnectionError::Forbidden);
                }
                self.stores
                    .sandbox_model
                    .access_request_connection_services(&caller.subject, id)
                    .await
                    .map_err(|error| match error {
                        layer2_core::sandbox::agent_access::AgentAccessError::NotFound => {
                            ConnectionError::NotFound
                        }
                        layer2_core::sandbox::agent_access::AgentAccessError::Conflict => {
                            ConnectionError::Changed
                        }
                        layer2_core::sandbox::agent_access::AgentAccessError::Forbidden => {
                            ConnectionError::Forbidden
                        }
                        _ => ConnectionError::Unavailable,
                    })
            }
            ConnectionTarget::SandboxProfile { profile } => {
                if !caller.authorization.allows(Capability::CreateSandbox) {
                    return Err(ConnectionError::Forbidden);
                }
                let visible = self
                    .stores
                    .sandbox_profiles
                    .sandbox_profile(&profile.id)
                    .await
                    .map_err(|_| ConnectionError::Unavailable)?
                    .filter(|profile| {
                        profile.retired_at.is_none()
                            && (profile.owner == caller.subject
                                || profile.visibility == ProfileVisibility::Public)
                    })
                    .ok_or(ConnectionError::NotFound)?;
                let revision = self
                    .stores
                    .sandbox_profiles
                    .sandbox_profile_revision(&visible.id, profile.revision)
                    .await
                    .map_err(|_| ConnectionError::Unavailable)?
                    .ok_or(ConnectionError::NotFound)?;
                self.profile_services(&revision.content).await
            }
            ConnectionTarget::Sandbox { id } => self.sandbox(caller, id).await,
            ConnectionTarget::Workspace { id } => {
                let workspace = self
                    .stores
                    .sandbox_model
                    .get_workspace(id)
                    .await
                    .map_err(|_| ConnectionError::Unavailable)?
                    .filter(|workspace| {
                        workspace.owner == caller.subject && workspace.removed_at.is_none()
                    })
                    .ok_or(ConnectionError::NotFound)?;
                self.sandbox(caller, &workspace.sandbox_id).await
            }
            ConnectionTarget::Agent { id } => {
                let agent = self
                    .stores
                    .sandbox_model
                    .get_agent(id)
                    .await
                    .map_err(|_| ConnectionError::Unavailable)?
                    .filter(|agent| {
                        agent.owner == caller.subject
                            && agent.status != layer2_core::sandbox::AgentStatus::Deleted
                    })
                    .ok_or(ConnectionError::NotFound)?;
                self.sandbox(caller, &agent.sandbox_id).await
            }
            ConnectionTarget::Launch { id } => {
                let launch = self
                    .stores
                    .trigger_definitions
                    .launch(id, &caller.subject)
                    .await
                    .map_err(|_| ConnectionError::Unavailable)?
                    .ok_or(ConnectionError::NotFound)?;
                if let Some(sandbox) = launch.resolution.sandbox.id() {
                    return self.sandbox(caller, sandbox).await;
                }
                let revision = self
                    .stores
                    .trigger_definitions
                    .trigger_revision(&launch.trigger_id, &launch.revision_digest)
                    .await
                    .map_err(|_| ConnectionError::Unavailable)?
                    .ok_or(ConnectionError::NotFound)?;
                let profiles = self
                    .stores
                    .sandbox_profiles
                    .read_trigger_profiles(&revision.content.target)
                    .await
                    .map_err(|_| ConnectionError::Unavailable)?;
                self.profile_services(&profiles.sandbox.content).await
            }
        }
    }
}

#[path = "targets_scope.rs"]
mod scope;
