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
        Ok(())
    }

    pub fn runtime_input(&self) -> layer3_adapter::ConversationInput {
        use layer3_adapter::{PermissionLevel as Permission, ReasoningEffort as Effort};
        layer3_adapter::ConversationInput::Configure {
            model: self.model.clone(),
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
        self.require_owner_authority(caller)?;
        settings
            .validate()
            .map_err(|error| AgentOperationError::InvalidRequest(error.to_string()))?;
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
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
            let at = now_rfc3339();
            let sequence = next_sequence(current.conversation.last_seq)?;
            let previous = current.conversation.settings.clone();
            let mut next = current.clone();
            next.conversation.settings = Some(settings.clone());
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
                    let mut recovery = SettingsRecoveryOnDrop {
                        runtimes: self.runtimes.clone(),
                        target: target.clone(),
                        armed: true,
                    };
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
        Err(AgentOperationError::Conflict)
    }
}

struct SettingsRecoveryOnDrop {
    runtimes: std::sync::Arc<dyn super::AgentRuntimeRegistry>,
    target: super::AgentRuntimeTarget,
    armed: bool,
}

impl Drop for SettingsRecoveryOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let runtimes = self.runtimes.clone();
            let target = self.target.clone();
            tokio::spawn(async move {
                let _ = runtimes.request_reconnect(&target).await;
            });
        }
    }
}
