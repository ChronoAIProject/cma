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
    authorized: Arc<AuthorizedResource>,
    target: ResourceTarget,
    settings: AgentSettings,
    preferences: Option<super::AgentPreferencesUpdate>,
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
        self.authorize_agent_configuration(context, credential, id, Some(settings), None)
            .await
    }

    pub async fn authorize_agent_preferences(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        preferences: &super::AgentPreferencesUpdate,
    ) -> Result<AgentSettingsAuthorization, ResourceAccessError> {
        self.authorize_agent_configuration(context, credential, id, None, Some(preferences))
            .await
    }

    async fn authorize_agent_configuration(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        settings: Option<&AgentSettings>,
        preferences: Option<&super::AgentPreferencesUpdate>,
    ) -> Result<AgentSettingsAuthorization, ResourceAccessError> {
        if !context.validate_principal()
            || !context
                .caller
                .authorization
                .allows(Capability::UseOwnSandboxes)
            || settings.is_some_and(|value| value.validate().is_err())
            || preferences.is_some_and(|value| value.validate().is_err())
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
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?;
        let mut settings = settings.cloned().unwrap_or_else(|| agent.settings());
        if settings.system_prompt.is_none() {
            settings.system_prompt = agent.settings().system_prompt;
        }
        if let Some(super::AgentPreferencesUpdate::SystemPrompt { system_prompt, .. }) = preferences
        {
            settings.system_prompt = Some(system_prompt.clone());
        }
        let fence = Arc::new(AgentSettingsFence {
            authorized: Arc::new(authorized),
            target,
            settings,
            preferences: preferences.cloned(),
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
    pub(super) fn preferences(&self) -> Option<&super::AgentPreferencesUpdate> {
        self.fence.preferences.as_ref()
    }

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
        let preference_revision = self.preferences.as_ref().is_none_or(|update| {
            update.expected_revision().checked_add(1)
                == Some(mutation.agent.conversation.preferences.revision)
        });
        let operation_matches = match &self.preferences {
            Some(super::AgentPreferencesUpdate::Title {title,..}) =>
                mutation.agent.conversation.preferences.title.as_ref() == Some(title)
                && mutation.agent_logs == vec![super::AgentLogKind::TitleSet {title:title.clone(), source:"owner_override".into()}],
            _ => mutation.agent.conversation.operation.as_ref().is_some_and(|operation| {
                operation.state == super::AgentOperationDelivery::DispatchFenced
                    && matches!(&operation.intent, super::AgentOperationIntent::UpdateSettings { settings, .. } if settings == &self.settings)
            }),
        };
        let valid = self.target
            == (ResourceTarget::Agent {
                agent_id: mutation.agent.id.clone(),
            })
            && preference_revision
            && operation_matches;
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
            skill_upgrades_enabled: self.skill_upgrades_enabled,
            instruction_policy: self.instruction_policy.clone(),
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

impl AgentOperationService {
    pub async fn update_preferences_authorized(
        &self,
        authority: &AgentSettingsAuthorization,
    ) -> Result<super::Agent, AgentOperationError> {
        authority.fence.require_current().map_err(operation_error)?;
        let crate::resource_access::ResourceTarget::Agent { agent_id } = &authority.fence.target
        else {
            return Err(AgentOperationError::Forbidden);
        };
        let update = authority
            .fence
            .preferences
            .clone()
            .ok_or(AgentOperationError::Forbidden)?;
        let mut authority = authority.clone();
        if let super::AgentPreferencesUpdate::SystemPrompt { system_prompt, .. } = &update {
            let agent = authority
                .fence
                .authorized
                .snapshot
                .lineage
                .agent
                .as_ref()
                .ok_or(AgentOperationError::NotFound)?;
            let mut fence = AgentSettingsFence {
                authorized: authority.fence.authorized.clone(),
                target: authority.fence.target.clone(),
                settings: authority.fence.settings.clone(),
                preferences: authority.fence.preferences.clone(),
                deadline: authority.fence.deadline,
            };
            fence.settings.system_prompt =
                Some(self.instruction_policy.compose(agent, system_prompt));
            authority.fence = Arc::new(fence);
        }
        let service = Self {
            settings_authority: Some(authority.clone()),
            command_claim: None,
            execution_authority: None,
            trigger_authority: None,
            skill_upgrades_enabled: self.skill_upgrades_enabled,
            instruction_policy: self.instruction_policy.clone(),
            connections: self.connections.clone(),
            store: self.store.clone(),
            runtimes: self.runtimes.clone(),
        };
        service
            .update_preferences(&authority.caller, agent_id, update)
            .await
    }
}

pub(super) fn operation_error(error: ResourceAccessError) -> AgentOperationError {
    super::agent_io::execution_authority::operation_error(error)
}
