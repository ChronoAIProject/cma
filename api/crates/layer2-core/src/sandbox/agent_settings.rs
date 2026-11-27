//! Owner-selected conversation settings, separate from immutable profile material.

use serde::{Deserialize, Serialize};

use crate::profiles::{PermissionLevel, ThinkingEffort};
use crate::{now_rfc3339, AgentId, Caller, RuntimeCapability};

use super::agent_operation_dispatch::{
    new_operation, next_sequence, operation_log, runtime_target, PreWriteRollback,
    PreparedAgentOperation,
};
use super::agent_operation_service::MAX_OPERATION_COMMIT_RETRIES;
use super::{
    Agent, AgentOperationError, AgentOperationIntent, AgentOperationService, AgentStatus,
    SandboxModelError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSettings {
    pub model: String,
    pub reasoning_effort: ThinkingEffort,
    pub permission_mode: PermissionLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

impl AgentSettings {
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        if self.model.is_empty()
            || self.model.len() > 256
            || self.model.trim() != self.model
            || self.model.chars().any(char::is_control)
        {
            return Err(SandboxModelError::invalid_value("agent.settings.model"));
        }
        if self
            .system_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.len() > 32 * 1024 || prompt.contains('\0'))
        {
            return Err(SandboxModelError::invalid_value(
                "agent.settings.system_prompt",
            ));
        }
        Ok(())
    }

    pub fn runtime_input(&self) -> layer3_adapter::ConversationInput {
        use layer3_adapter::{PermissionLevel as Permission, ReasoningEffort as Effort};
        layer3_adapter::ConversationInput::Configure {
            model: self.model.clone(),
            system_prompt: self.system_prompt.clone(),
            reasoning_effort: match self.reasoning_effort {
                ThinkingEffort::Minimal => Effort::Minimal,
                ThinkingEffort::Low => Effort::Low,
                ThinkingEffort::Medium => Effort::Medium,
                ThinkingEffort::High => Effort::High,
                ThinkingEffort::XHigh => Effort::XHigh,
            },
            permission: match self.permission_mode {
                PermissionLevel::Guarded => Permission::Guarded,
                PermissionLevel::Autonomous => Permission::Autonomous,
            },
        }
    }
}

impl Agent {
    pub fn selected_model(&self) -> Option<String> {
        let model = self.settings().model;
        (!model.is_empty()).then_some(model)
    }

    pub fn settings(&self) -> AgentSettings {
        self.conversation
            .settings
            .clone()
            .unwrap_or_else(|| AgentSettings {
                // Legacy profiles may defer model choice to the runtime. Empty model
                // accurately represents that state; updates always name an exact model.
                model: self.profile.content.model.clone().unwrap_or_default(),
                reasoning_effort: self.profile.content.thinking_effort,
                permission_mode: self.profile.content.permission_level,
                system_prompt: None,
            })
    }

    pub fn settings_pending(&self) -> bool {
        self.conversation
            .operation
            .as_ref()
            .is_some_and(|operation| {
                matches!(
                    operation.intent,
                    AgentOperationIntent::UpdateSettings { .. }
                ) && operation.blocks_dispatch()
            })
    }
}

impl AgentOperationService {
    /// Apply an absolute configuration while the conversation is idle. The
    /// dispatch fence and desired settings become durable before the first RPC.
    pub async fn update_settings(
        &self,
        caller: &Caller,
        id: &AgentId,
        settings: AgentSettings,
    ) -> Result<Agent, AgentOperationError> {
        self.update_settings_inner(caller, id, settings, None).await
    }

    pub(super) async fn update_settings_with_revision(
        &self,
        caller: &Caller,
        id: &AgentId,
        settings: AgentSettings,
        expected_revision: u64,
    ) -> Result<Agent, AgentOperationError> {
        self.update_settings_inner(caller, id, settings, Some(expected_revision))
            .await
    }

    async fn update_settings_inner(
        &self,
        caller: &Caller,
        id: &AgentId,
        settings: AgentSettings,
        expected_revision: Option<u64>,
    ) -> Result<Agent, AgentOperationError> {
        self.require_owner_authority(caller)?;
        settings
            .validate()
            .map_err(|error| AgentOperationError::InvalidRequest(error.to_string()))?;
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
            let mut settings = settings.clone();
            if settings.system_prompt.is_none() {
                settings.system_prompt = current.settings().system_prompt;
            }
            self.validate_preferences_revision(&current)?;
            if expected_revision
                .is_some_and(|revision| revision != current.conversation.preferences.revision)
            {
                return Err(AgentOperationError::PreferencesRevisionMismatch);
            }
            if current.work.step != super::AgentWorkStep::None
                || current.status != AgentStatus::Ready
                || current.conversation.active_turn.is_some()
                || current
                    .conversation
                    .queued_messages
                    .iter()
                    .any(|message| message.state != super::QueuedMessageState::Delivered)
            {
                return Err(AgentOperationError::StatusConflict {
                    status: current.status,
                });
            }
            self.require_first_message_settled(&current)?;
            self.require_dispatch_available(&current)?;
            current
                .capabilities
                .require([RuntimeCapability::Conversation])
                .map_err(|capability| AgentOperationError::CapabilityUnsupported { capability })?;
            if current.settings() == settings {
                return Ok(current);
            }
            let (parent, queues) = self.parent_admission(&current).await?;
            if queues {
                return Err(AgentOperationError::StatusConflict {
                    status: current.status,
                });
            }
            let target = runtime_target(&current)?;
            let required = vec![RuntimeCapability::Conversation];
            self.require_runtime(&target, &required).await?;
            if settings.system_prompt != current.settings().system_prompt
                && !self.system_prompt_supported(&current).await
            {
                return Err(AgentOperationError::InvalidRequest(
                    "Update this Sandbox runtime when idle before changing Agent instructions"
                        .into(),
                ));
            }
            let at = now_rfc3339();
            let sequence = next_sequence(current.conversation.last_seq)?;
            let previous = current.conversation.settings.clone();
            let mut next = current.clone();
            next.conversation.settings = Some(settings.clone());
            if settings.system_prompt != current.settings().system_prompt {
                next.conversation.preferences.revision = current
                    .conversation
                    .preferences
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| {
                        AgentOperationError::Corrupt("preferences revision overflow".into())
                    })?;
            }
            next.conversation.operation = Some(new_operation(
                sequence,
                &target,
                sequence,
                AgentOperationIntent::UpdateSettings {
                    settings: settings.clone(),
                    previous: previous.clone(),
                },
                &at,
            )?);
            let logs = vec![operation_log(&next, sequence)?];
            match self
                .commit_admission(&current, next, logs, at, parent)
                .await
            {
                Ok(_) => {
                    let mut recovery = super::agent_runtime::ReconnectOnDrop::new(
                        self.runtimes.clone(),
                        target.clone(),
                    );
                    let result = self
                        .dispatch_prepared(PreparedAgentOperation {
                            target,
                            operation_sequence: sequence,
                            turn_sequence: sequence,
                            required,
                            input: settings.runtime_input(),
                            rollback: PreWriteRollback::Settings(previous),
                            queued_message_sequence: None,
                        })
                        .await;
                    recovery.armed = matches!(
                        result,
                        Err(AgentOperationError::DeliveryAmbiguous { .. }
                            | AgentOperationError::OutcomeUnavailable(_))
                    );
                    return result.map(|receipt| receipt.agent);
                }
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::ConfigurationAdmissionConflict)
    }
}
