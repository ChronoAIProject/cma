use serde::{Deserialize, Serialize};

use super::{Agent, AgentLogEntry, AgentLogKind, AgentStatus};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskStatus {
    NotStarted,
    Completed,
    Working,
    WaitingApproval,
    Starting,
    Paused,
    Stopped,
    Failed,
    #[default]
    Unknown,
}

impl AgentTaskStatus {
    pub fn in_sandbox(self, sandbox: &super::Sandbox) -> Self {
        if matches!(self, Self::Failed | Self::Stopped) {
            return self;
        }
        if let Some(operation) = &sandbox.lifecycle.operation {
            return match operation {
                super::SandboxLifecycleOperation::Pause { .. } => Self::Paused,
                _ => Self::Starting,
            };
        }
        match sandbox.status {
            super::SandboxStatus::Active | super::SandboxStatus::CredentialRotating => self,
            super::SandboxStatus::Paused | super::SandboxStatus::Pausing => Self::Paused,
            super::SandboxStatus::Failed => Self::Failed,
            super::SandboxStatus::Retiring | super::SandboxStatus::Retired => Self::Stopped,
            _ => Self::Starting,
        }
    }

    pub fn relevant(kind: &AgentLogKind) -> bool {
        matches!(
            kind,
            AgentLogKind::Created { .. }
                | AgentLogKind::TurnStarted { .. }
                | AgentLogKind::TurnCompleted { .. }
                | AgentLogKind::TurnFailed { .. }
                | AgentLogKind::TurnInterrupted { .. }
                | AgentLogKind::ConversationGap { .. }
        )
    }

    pub fn from_log(agent: &Agent, entries: &[AgentLogEntry], pending_work: bool) -> Self {
        let latest = entries
            .iter()
            .rev()
            .find(|entry| Self::relevant(&entry.kind));
        let gap = entries
            .iter()
            .rev()
            .find(|entry| matches!(entry.kind, AgentLogKind::ConversationGap { .. }));
        Self::from_latest(agent, latest, gap, pending_work)
    }

    pub fn from_latest(
        agent: &Agent,
        latest: Option<&AgentLogEntry>,
        gap: Option<&AgentLogEntry>,
        pending_work: bool,
    ) -> Self {
        match agent.status {
            AgentStatus::Starting => return Self::Starting,
            AgentStatus::Working => return Self::Working,
            AgentStatus::WaitingApproval => return Self::WaitingApproval,
            AgentStatus::Paused => return Self::Paused,
            AgentStatus::Stopped | AgentStatus::Deleted => return Self::Stopped,
            AgentStatus::Failed => return Self::Failed,
            AgentStatus::Ready => (),
        }
        if pending_work
            || agent.conversation.active_turn.is_some()
            || agent
                .conversation
                .queued_messages
                .iter()
                .any(|message| message.state != super::QueuedMessageState::Delivered)
            || agent
                .conversation
                .operation
                .as_ref()
                .is_some_and(|operation| {
                    matches!(
                        operation.state,
                        super::AgentOperationDelivery::DispatchFenced
                            | super::AgentOperationDelivery::WriteConfirmed
                            | super::AgentOperationDelivery::RecoveryRequired
                    )
                })
            || agent
                .conversation
                .first_message
                .as_ref()
                .is_some_and(|message| !message.is_terminal())
        {
            return Self::Working;
        }
        let Some(latest) = latest
            .filter(|entry| entry.agent_id == agent.id && entry.seq <= agent.conversation.last_seq)
        else {
            return Self::Unknown;
        };
        match latest.kind {
            AgentLogKind::Created { .. } => Self::NotStarted,
            AgentLogKind::TurnCompleted { turn_sequence } => {
                if gap.is_some_and(|gap| {
                    gap.agent_id == agent.id
                        && gap.seq > turn_sequence
                        && gap.seq < latest.seq
                        && matches!(gap.kind, AgentLogKind::ConversationGap { .. })
                }) {
                    return Self::Failed;
                }
                match agent
                    .conversation
                    .first_message
                    .as_ref()
                    .filter(|message| message.turn_sequence == Some(turn_sequence))
                {
                    Some(message) => match message.purpose {
                        Some(super::FirstMessagePurpose::Setup) => Self::NotStarted,
                        Some(super::FirstMessagePurpose::Task) => Self::Completed,
                        None => Self::Unknown,
                    },
                    None => Self::Completed,
                }
            }
            AgentLogKind::TurnFailed { .. } => Self::Failed,
            AgentLogKind::TurnInterrupted { .. } => Self::Stopped,
            AgentLogKind::TurnStarted { .. } => Self::Working,
            _ => Self::Unknown,
        }
    }
}
