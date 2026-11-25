use super::AgentWorkStep;
use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};
use crate::profiles::AgentProfileSnapshot;
use crate::{RuntimeCapabilities, RuntimeCapability};

use super::{
    AgentOperationRecord, AgentStatus, AgentWorkState, FirstMessageRecord, QueuedAgentMessage,
    SandboxModelError, TypedFailure, Worktree, WorktreeStatus, MAX_QUEUED_AGENT_MESSAGES,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationState {
    #[serde(default, skip_serializing_if = "super::AgentObservations::is_empty")]
    pub observations: super::AgentObservations,
    /// Durable owner overrides; published profiles and open receipt identities stay immutable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<super::AgentSettings>,
    #[serde(default, skip_serializing_if = "super::AgentPreferences::is_empty")]
    pub preferences: super::AgentPreferences,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub managed_guidance_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_generation: Option<u64>,
    pub last_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_message: Option<FirstMessageRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_timeout: Option<super::turn_timeout::TurnTimeout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<AgentOperationRecord>,
    #[serde(default)]
    pub queued_messages: Vec<QueuedAgentMessage>,
}

impl ConversationState {
    pub(super) fn interrupt_active_turn(&mut self) -> Option<super::AgentLogKind> {
        let turn_sequence = self.active_turn.take()?;
        let terminal = super::AgentLogKind::TurnInterrupted { turn_sequence };
        if let Some(task) = self.observations.task_acceptance.as_mut() {
            task.observe_execution(Some(turn_sequence), std::slice::from_ref(&terminal));
        }
        Some(terminal)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStopReason {
    Owner,
    Operator,
    RuntimeLost,
    SandboxRetired,
    BindingRetired,
    Failed,
}

impl AgentStopReason {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Operator => "operator",
            Self::RuntimeLost => "runtime_lost",
            Self::SandboxRetired => "sandbox_retired",
            Self::BindingRetired => "binding_retired",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentOrigin {
    Manual,
    Lifecycle {
        operation_id: String,
    },
    WorkspaceProfile {
        index: u16,
    },
    Trigger {
        trigger_id: String,
        launch_id: String,
    },
}

pub(crate) fn profile_agent_creation_key(workspace_id: &WorkspaceId, index: u16) -> String {
    format!("workspace-profile:{workspace_id}:{index}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    pub id: AgentId,
    pub worktree_id: WorktreeId,
    pub workspace_id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub creation_key: String,
    pub creation_fingerprint: String,
    pub profile: AgentProfileSnapshot,
    /// Runtime abilities granted when the Agent was admitted. This set is
    /// durable and never expands when a later runtime advertises more.
    #[serde(default)]
    pub capabilities: RuntimeCapabilities,
    pub status: AgentStatus,
    pub conversation: ConversationState,
    pub work: AgentWorkState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<TypedFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<String>,
    /// Durable owner intent consumed when the worker publishes `Stopped`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_stop_reason: Option<AgentStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<AgentStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
    pub origin: AgentOrigin,
    pub revision: u64,
}

impl Agent {
    /// Derive the terminal event once the caller has proved the current native Close.
    /// The command record still carries the exact delivery outcome before the Stop.
    pub fn settle_owner_stop_turn(&mut self) -> Option<super::AgentLogKind> {
        if self.work.step != AgentWorkStep::PublishStopped
            || self.requested_stop_reason != Some(AgentStopReason::Owner)
            || !self
                .work
                .receipts
                .iter()
                .any(|receipt| receipt.step == AgentWorkStep::CloseProcess)
        {
            return None;
        }
        self.conversation.interrupt_active_turn()
    }

    pub fn validate_for_admission(&self, worktree: &Worktree) -> Result<(), SandboxModelError> {
        self.validate_lineage(worktree)?;
        if self.work.step == AgentWorkStep::WaitForWorkspace
            && (!self.has_no_runtime_effects() || !worktree.base_commit.is_empty())
        {
            return Err(SandboxModelError::invalid_value("agent.bootstrap"));
        }
        if worktree.status != WorktreeStatus::Creating
            || worktree.agent_id.as_ref() != Some(&self.id)
            || self.status != AgentStatus::Starting
            || self.revision != 1
            || self.creation_key.trim().is_empty()
            || self.creation_key.len() > 256
            || self.creation_fingerprint.len() != 64
            || !self
                .creation_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || !self.capabilities.supports(RuntimeCapability::Conversation)
            || self.failure.is_some()
            || self.conversation.conversation_id.is_some()
            || self.conversation.runtime_generation.is_some()
            || self.conversation.last_seq != 1
            || self.conversation.active_turn.is_some()
            || self.conversation.turn_timeout.is_some()
            || self.conversation.settings.is_some()
            || !self.conversation.preferences.is_empty()
            || self.conversation.managed_guidance_revision != 0
            || !self.conversation.observations.is_empty()
            || self.conversation.operation.is_some()
            || !self.conversation.queued_messages.is_empty()
            || self.work.next_fence == 0
            || self.work.resolved_skills.is_some()
            || !self.work.worktree_recoveries.is_empty()
        {
            return Err(SandboxModelError::invalid_value("agent.admission"));
        }
        if let Some(first_message) = self.conversation.first_message.as_ref() {
            first_message.validate()?;
        }
        Ok(())
    }

    pub fn validate_prerequisites(&self, worktree: &Worktree) -> Result<(), SandboxModelError> {
        self.validate_lineage(worktree)?;
        if worktree.status != WorktreeStatus::Ready {
            return Err(SandboxModelError::Prerequisite {
                resource: "agent",
                detail: "worktree is not ready",
            });
        }
        if worktree.agent_id.as_ref() != Some(&self.id) {
            return Err(SandboxModelError::Prerequisite {
                resource: "agent",
                detail: "worktree is already assigned",
            });
        }
        if self.revision == 0 || self.work.next_fence == 0 {
            return Err(SandboxModelError::invalid_value("agent.revision"));
        }
        self.validate_conversation()?;
        Ok(())
    }

    pub fn validate_conversation(&self) -> Result<(), SandboxModelError> {
        self.validate_worktree_recoveries()?;
        if self
            .conversation
            .turn_timeout
            .as_ref()
            .is_some_and(|timeout| {
                timeout.validate().is_err() || !timeout.binding().matches_incarnation(self)
            })
        {
            return Err(SandboxModelError::invalid_value("agent.turn_timeout"));
        }
        let identity_shape = match (
            self.conversation.conversation_id.as_deref(),
            self.conversation.runtime_generation,
        ) {
            (None, None) => true,
            (Some(id), Some(generation)) => {
                !id.trim().is_empty() && id.len() <= 512 && generation > 0
            }
            _ => false,
        };
        let requires_identity =
            matches!(
                self.status,
                AgentStatus::Ready
                    | AgentStatus::Working
                    | AgentStatus::WaitingApproval
                    | AgentStatus::Paused
            ) || (matches!(self.status, AgentStatus::Stopped | AgentStatus::Deleted)
                && self.ready_at.is_some());
        let queue_is_valid = self.conversation.queued_messages.len() <= MAX_QUEUED_AGENT_MESSAGES
            && self
                .conversation
                .queued_messages
                .iter()
                .enumerate()
                .all(|(index, message)| {
                    message.validate().is_ok()
                        && (index == 0
                            || self.conversation.queued_messages[index - 1].sequence
                                < message.sequence)
                });
        if self
            .conversation
            .settings
            .as_ref()
            .is_some_and(|settings| settings.validate().is_err())
            || self.conversation.preferences.validate().is_err()
            || !identity_shape
            || (requires_identity && self.conversation.conversation_id.is_none())
            || self
                .conversation
                .active_turn
                .is_some_and(|turn| turn == 0 || self.status == AgentStatus::Ready)
            || !queue_is_valid
            || self
                .conversation
                .first_message
                .as_ref()
                .is_some_and(|record| record.validate().is_err())
            || self.conversation.operation.as_ref().is_some_and(|record| {
                record.validate().is_err() || record.sequence > self.conversation.last_seq
            })
        {
            return Err(SandboxModelError::invalid_value("agent.conversation"));
        }
        Ok(())
    }

    pub fn transition(
        &mut self,
        next: AgentStatus,
        at: String,
        reason: Option<AgentStopReason>,
    ) -> Result<(), SandboxModelError> {
        if (next == AgentStatus::Stopped) != reason.is_some() {
            return Err(SandboxModelError::invalid_value("agent.stop_reason"));
        }
        if next == AgentStatus::Stopped
            && self
                .requested_stop_reason
                .as_ref()
                .is_some_and(|requested| Some(requested) != reason.as_ref())
        {
            return Err(SandboxModelError::invalid_value("agent.stop_reason"));
        }
        let status = self.status.transition(next)?;
        if next == AgentStatus::Ready
            && (self.conversation.conversation_id.is_none()
                || self.conversation.runtime_generation.is_none())
        {
            return Err(SandboxModelError::invalid_value("agent.conversation_id"));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("agent.revision"))?;
        self.status = status;
        self.updated_at = at.clone();
        match next {
            AgentStatus::Ready if self.ready_at.is_none() => self.ready_at = Some(at),
            AgentStatus::Stopped => {
                self.requested_stop_reason = None;
                self.stop_reason = reason;
                self.stopped_at = Some(at);
            }
            AgentStatus::Deleted => self.deleted_at = Some(at),
            _ => {}
        }
        self.revision = revision;
        Ok(())
    }

    /// Persist stop intent and fence prior lifecycle authority without publishing `Stopped`.
    pub fn request_stop(
        &mut self,
        reason: AgentStopReason,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if !self.status.is_live()
            || self.requested_stop_reason.is_some()
            || self.stop_reason.is_some()
        {
            return Err(SandboxModelError::invalid_value(
                "agent.requested_stop_reason",
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("agent.revision"))?;
        self.requested_stop_reason = Some(reason);
        if let Some(skill) = self
            .work
            .live_skills
            .last_mut()
            .filter(|s| s.status == super::AgentSkillInstallStatus::Pending)
        {
            skill.status = super::AgentSkillInstallStatus::Failed;
            skill.error = Some("agent_stopped_before_skill_confirmation".into());
        }
        self.updated_at = at.clone();
        // A published process can be stopped through the runtime's serialized,
        // durable CloseAgent fence. A stale connection/publication lease is not
        // an outstanding filesystem writer and must not delay shutdown.
        let process_is_published = self.conversation.conversation_id.is_some()
            && self.conversation.runtime_generation.is_some()
            && self.work.cleanup_debt.is_empty()
            && self
                .work
                .receipts
                .iter()
                .any(|receipt| receipt.step == AgentWorkStep::OpenConversation)
            && matches!(
                self.work.step,
                AgentWorkStep::PublishReady
                    | AgentWorkStep::PublishConnection
                    | AgentWorkStep::DispatchFirstMessage
                    | AgentWorkStep::None
            );
        // Workspace readiness only observes the parents before a fenced
        // publication. Stop can supersede that claim without a runtime wait.
        let readiness_only =
            self.work.step == AgentWorkStep::WaitForWorkspace && self.has_no_runtime_effects();
        let cleanup_not_before = self
            .work
            .lease
            .as_ref()
            .filter(|lease| !process_is_published && !readiness_only && lease.expires_at > at)
            .map(|lease| lease.expires_at.clone());
        self.work
            .schedule(super::AgentWorkStep::FenceInteraction, at);
        self.work.cleanup_not_before = cleanup_not_before;
        self.revision = revision;
        Ok(())
    }

    /// Schedule terminal cleanup without publishing `Deleted`.
    pub fn request_delete(&mut self, at: String) -> Result<(), SandboxModelError> {
        if !matches!(self.status, AgentStatus::Stopped | AgentStatus::Failed)
            || self.requested_stop_reason.is_some()
        {
            return Err(SandboxModelError::invalid_value("agent.delete_status"));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("agent.revision"))?;
        self.updated_at = at.clone();
        self.work
            .schedule(super::AgentWorkStep::EnsureProcessClosed, at);
        self.revision = revision;
        Ok(())
    }

    pub fn fail(&mut self, failure: TypedFailure, at: String) -> Result<(), SandboxModelError> {
        if self.status != AgentStatus::Starting
            || failure.class.trim().is_empty()
            || failure.detail.trim().is_empty()
        {
            return Err(SandboxModelError::invalid_value("agent.failure"));
        }
        self.status = self.status.transition(AgentStatus::Failed)?;
        self.failure = Some(failure);
        self.updated_at = at;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("agent.revision"))?;
        Ok(())
    }

    /// Select the process named by the admitted Open, including an uncertain Open
    /// that still needs cleanup. The initial incarnation keeps its legacy ID.
    pub fn close_process_request(
        &self,
    ) -> Result<layer3_adapter::AgentCloseRequest, SandboxModelError> {
        use layer3_adapter::{AgentCloseRequest, AgentProcessAction, AgentSlot};

        let incarnation = self
            .open_process_incarnation()?
            .ok_or_else(|| SandboxModelError::invalid_value("agent.work.open_process"))?;
        Ok(AgentCloseRequest::new(
            AgentProcessAction::Close.operation_id(self.id.as_str(), incarnation),
            AgentSlot::new(self.id.as_str()),
        ))
    }

    pub(super) fn open_process_incarnation(&self) -> Result<Option<u64>, SandboxModelError> {
        let mut opens = self
            .work
            .receipts
            .iter()
            .filter(|receipt| receipt.step == AgentWorkStep::OpenConversation)
            .map(|receipt| receipt.operation_id.as_str())
            .chain(
                self.work
                    .cleanup_debt
                    .iter()
                    .filter(|debt| debt.step == AgentWorkStep::OpenConversation)
                    .map(|debt| debt.operation_id.as_str()),
            );
        let invalid = || SandboxModelError::invalid_value("agent.work.open_process");
        let Some(operation) = opens.next() else {
            return Ok(None);
        };
        if opens.next().is_some() {
            return Err(invalid());
        }
        layer3_adapter::AgentProcessAction::Open
            .incarnation(self.id.as_str(), operation)
            .map(Some)
            .ok_or_else(invalid)
    }

    pub fn operation_id(&self, step: super::AgentWorkStep) -> String {
        format!("agent:{}:{}:v1", self.id, step.operation_name())
    }

    fn validate_lineage(&self, worktree: &Worktree) -> Result<(), SandboxModelError> {
        if worktree.id != self.worktree_id
            || worktree.workspace_id != self.workspace_id
            || worktree.sandbox_id != self.sandbox_id
            || worktree.owner != self.owner
        {
            return Err(SandboxModelError::Prerequisite {
                resource: "agent",
                detail: "worktree lineage does not match",
            });
        }
        Ok(())
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}
