use serde::{Deserialize, Serialize};

use super::SandboxModelError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentWorkStep {
    AddWorktree,
    MaterializeHome,
    OpenConversation,
    PublishReady,
    DispatchFirstMessage,
    FenceInteraction,
    CloseProcess,
    PublishStopped,
    EnsureProcessClosed,
    RemoveHome,
    RemoveWorktree,
    PublishDeleted,
    None,
}

impl AgentWorkStep {
    pub const fn is_pending(self) -> bool {
        !matches!(self, Self::None)
    }

    pub const fn operation_name(self) -> &'static str {
        match self {
            Self::AddWorktree => "add_worktree",
            Self::MaterializeHome => "materialize_home",
            Self::OpenConversation => "open_conversation",
            Self::PublishReady => "publish_ready",
            Self::DispatchFirstMessage => "dispatch_first_message",
            Self::FenceInteraction => "fence_interaction",
            Self::CloseProcess => "close_process",
            Self::PublishStopped => "publish_stopped",
            Self::EnsureProcessClosed => "ensure_process_closed",
            Self::RemoveHome => "remove_home",
            Self::RemoveWorktree => "remove_worktree",
            Self::PublishDeleted => "publish_deleted",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWorkLease {
    pub holder: String,
    pub fence: u64,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWorkReceipt {
    pub step: AgentWorkStep,
    pub operation_id: String,
    pub request_sha256: String,
    pub completed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCleanupDebt {
    pub step: AgentWorkStep,
    pub operation_id: String,
    pub request_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWorkState {
    pub step: AgentWorkStep,
    pub ready_at: String,
    pub next_fence: u64,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<AgentWorkLease>,
    #[serde(default)]
    pub receipts: Vec<AgentWorkReceipt>,
    #[serde(default)]
    pub cleanup_debt: Vec<AgentCleanupDebt>,
}

impl AgentWorkState {
    pub fn new(step: AgentWorkStep, ready_at: String) -> Self {
        Self {
            step,
            ready_at,
            next_fence: 1,
            attempts: 0,
            lease: None,
            receipts: Vec::new(),
            cleanup_debt: Vec::new(),
        }
    }

    pub fn is_due(&self, now: &str) -> bool {
        self.step.is_pending()
            && self.ready_at.as_str() <= now
            && self
                .lease
                .as_ref()
                .is_none_or(|lease| lease.expires_at.as_str() <= now)
    }

    pub fn claim(
        &mut self,
        holder: String,
        now: &str,
        expires_at: String,
    ) -> Result<u64, SandboxModelError> {
        if holder.trim().is_empty() || expires_at.as_str() <= now || !self.is_due(now) {
            return Err(SandboxModelError::invalid_value("agent.work.lease"));
        }
        let fence = self.next_fence;
        self.next_fence = fence
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("agent.work.next_fence"))?;
        self.lease = Some(AgentWorkLease {
            holder,
            fence,
            expires_at,
        });
        Ok(fence)
    }

    pub fn owns(&self, holder: &str, fence: u64) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|lease| lease.holder == holder && lease.fence == fence)
    }

    pub fn schedule(&mut self, step: AgentWorkStep, ready_at: String) {
        self.step = step;
        self.ready_at = ready_at;
        self.attempts = 0;
        self.lease = None;
    }

    pub fn advance(
        &mut self,
        holder: &str,
        fence: u64,
        next: AgentWorkStep,
        ready_at: String,
    ) -> Result<(), SandboxModelError> {
        if !self.owns(holder, fence) {
            return Err(SandboxModelError::invalid_value("agent.work.fence"));
        }
        self.schedule(next, ready_at);
        Ok(())
    }

    pub fn retry(
        &mut self,
        holder: &str,
        fence: u64,
        ready_at: String,
    ) -> Result<(), SandboxModelError> {
        if !self.owns(holder, fence) {
            return Err(SandboxModelError::invalid_value("agent.work.fence"));
        }
        self.attempts = self
            .attempts
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("agent.work.attempts"))?;
        self.ready_at = ready_at;
        self.lease = None;
        Ok(())
    }
}
