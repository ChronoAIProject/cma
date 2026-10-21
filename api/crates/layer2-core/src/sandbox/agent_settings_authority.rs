//! Current, nonrenewable authority for one absolute settings update.

use std::{sync::Arc, time::SystemTime};

use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceAccessSnapshot, ResourceAccessStoreError, ResourceCapability,
    ResourceTarget, MAX_MEMBERSHIP_AGE,
};
use crate::{AgentId, Caller, Capability};

use super::{AgentOperationError, AgentOperationService, AgentSettings};

#[derive(Clone)]
pub struct AgentSettingsAuthorization {
    caller: Caller,
    access: ResourceAccessService,
    fence: Arc<AgentSettingsFence>,
}

/// Consumed by the conversation store in the same transaction as the settings
/// intent. A retained HTTP response or old share never creates this proof.
pub struct AgentSettingsFence {
    authorized: AuthorizedResource,
    target: ResourceTarget,
    settings: AgentSettings,
    deadline: tokio::time::Instant,
}

impl std::fmt::Debug for AgentSettingsFence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSettingsFence")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl ResourceAccessService {
    pub async fn authorize_agent_settings(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        settings: &AgentSettings,
    ) -> Result<AgentSettingsAuthorization, ResourceAccessError> {
        if !context.validate_principal()
            || !context
                .caller
                .authorization
                .allows(Capability::UseOwnSandboxes)
            || settings.validate().is_err()
        {
            return Err(AccessError::Forbidden.into());
        }
        let started_at = SystemTime::now();
        let deadline = tokio::time::Instant::now() + MAX_MEMBERSHIP_AGE;
        let deadline = context
            .credential_deadline()
            .map_or(deadline, |expiry| deadline.min(expiry));
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        let authorized = self
            .authorize(
                context,
                credential,
                &target,
                ResourceCapability::OperateAgent,
                AccessPurpose::Mutation { started_at },
            )
            .await?;
        let deadline = authorized
            .decision
            .revalidation_deadline(ResourceCapability::OperateAgent)
            .map_or(deadline, |expiry| deadline.min(expiry));
        let fence = Arc::new(AgentSettingsFence {
            authorized,
            target,
            settings: settings.clone(),
            deadline,
        });
        fence.require_current()?;
        Ok(AgentSettingsAuthorization {
            caller: context.caller.clone(),
            access: self.clone(),
            fence,
        })
    }
}

impl AgentSettingsAuthorization {
    pub(super) fn caller(&self) -> &Caller {
        &self.caller
    }
    pub(super) fn fence(&self) -> Arc<AgentSettingsFence> {
        self.fence.clone()
    }

    pub(super) async fn current_snapshot(
        &self,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessError> {
        self.fence.require_current()?;
        let current = tokio::time::timeout_at(
            self.fence.deadline,
            self.access.current_read_snapshot(
                &self.fence.authorized,
                &self.fence.target,
                ResourceCapability::OperateAgent,
            ),
        )
        .await
        .map_err(|_| AccessError::RevalidationRequired)??;
        self.fence.validate_snapshot(&current)?;
        Ok(current)
    }
}

impl AgentSettingsFence {
    pub fn target(&self) -> &ResourceTarget {
        &self.target
    }

    pub fn require_current(&self) -> Result<(), ResourceAccessError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(AccessError::RevalidationRequired.into());
        }
        self.authorized
            .decision
            .require(ResourceCapability::OperateAgent)?;
        Ok(())
    }

    pub fn validate_snapshot(
        &self,
        current: &ResourceAccessSnapshot,
    ) -> Result<(), ResourceAccessError> {
        self.require_current()?;
        let original = &self.authorized.snapshot;
        if current.fence() != original.fence()
            || current.scope(&self.target)? != original.scope(&self.target)?
            || !super::agent_io::execution_authority::same_ancestry(
                &original.lineage,
                &current.lineage,
            )
        {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        Ok(())
    }

    pub(crate) fn validate_mutation(
        &self,
        mutation: &super::store::AgentConversationMutation,
    ) -> Result<(), super::store::SandboxStoreError> {
        let valid = self.target == (ResourceTarget::Agent { agent_id: mutation.agent.id.clone() })
            && mutation.agent.conversation.operation.as_ref().is_some_and(|operation| {
                operation.state == super::AgentOperationDelivery::DispatchFenced
                    && matches!(&operation.intent, super::AgentOperationIntent::UpdateSettings { settings, .. } if settings == &self.settings)
            });
        if self.require_current().is_err() || !valid {
            return Err(super::store::SandboxStoreError::Conflict);
        }
        Ok(())
    }
}

impl AgentOperationService {
    /// Preserve the real caller and the existing durable Configure intent;
    /// recheck the same share before admission and at the ordered runtime write.
    pub async fn update_settings_authorized(
        &self,
        authority: &AgentSettingsAuthorization,
    ) -> Result<super::Agent, AgentOperationError> {
        authority.fence.require_current().map_err(operation_error)?;
        let ResourceTarget::Agent { agent_id } = &authority.fence.target else {
            return Err(AgentOperationError::Forbidden);
        };
        let service = Self {
            settings_authority: Some(authority.clone()),
            command_claim: None,
            execution_authority: None,
            trigger_authority: None,
            connections: self.connections.clone(),
            store: self.store.clone(),
            runtimes: self.runtimes.clone(),
        };
        service
            .update_settings(
                &authority.caller,
                agent_id,
                authority.fence.settings.clone(),
            )
            .await
    }
}

pub(super) fn operation_error(error: ResourceAccessError) -> AgentOperationError {
    super::agent_io::execution_authority::operation_error(error)
}
