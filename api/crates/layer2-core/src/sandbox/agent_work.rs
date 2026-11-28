use serde::{Deserialize, Serialize};

use crate::ids::SkillId;
use crate::profiles::{SkillRef, SkillSource};

use super::SandboxModelError;

/// Immutable skill selection admitted with the first home-materialization intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedAgentSkill {
    pub source: SkillSource,
    pub id: SkillId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub version: String,
    pub archive_sha256: String,
}

impl ResolvedAgentSkill {
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        if self.id.as_str().is_empty()
            || self.id.as_str().len() > 256
            || self.name.as_ref().is_some_and(|name| {
                name.is_empty()
                    || name.len() > 256
                    || name.trim() != name
                    || name.chars().any(char::is_control)
            })
            || self.version.is_empty()
            || self.version.len() > 256
            || self.version.trim() != self.version
            || self.version.chars().any(char::is_control)
            || self.archive_sha256.len() != 64
            || !self
                .archive_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(SandboxModelError::invalid_value(
                "agent.work.resolved_skills",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentWorkStep {
    InstallSkill,
    /// The reserved Worktree has no base commit until its exact Workspace is ready.
    WaitForWorkspace,
    AddWorktree,
    MaterializeHome,
    OpenConversation,
    PublishReady,
    PublishConnection,
    DispatchFirstMessage,
    FenceInteraction,
    CloseProcess,
    PublishStopped,
    EnsureProcessClosed,
    RemoveHome,
    RemoveWorktree,
    PublishDeleted,
    RestartConversation,
    RestartTeardown,
    None,
}

impl AgentWorkStep {
    /// Only background work may wait for a receiver cleanup retry. New control
    /// steps and unknown steps must remain discoverable on the same Agent ID.
    pub const RECEIVER_PACED: [Self; 8] = [
        Self::InstallSkill,
        Self::WaitForWorkspace,
        Self::AddWorktree,
        Self::MaterializeHome,
        Self::OpenConversation,
        Self::PublishReady,
        Self::PublishConnection,
        Self::DispatchFirstMessage,
    ];

    pub const fn is_pending(self) -> bool {
        !matches!(self, Self::None)
    }

    pub const fn fences_conversation_output(self) -> bool {
        matches!(self, Self::RestartConversation | Self::RestartTeardown)
    }

    pub const fn operation_name(self) -> &'static str {
        match self {
            Self::InstallSkill => "install_skill",
            Self::WaitForWorkspace => "wait_for_workspace",
            Self::AddWorktree => "add_worktree",
            Self::MaterializeHome => "materialize_home",
            Self::OpenConversation => "open_conversation",
            Self::PublishReady => "publish_ready",
            Self::PublishConnection => "publish_connection",
            Self::DispatchFirstMessage => "dispatch_first_message",
            Self::FenceInteraction => "fence_interaction",
            Self::CloseProcess => "close_process",
            Self::PublishStopped => "publish_stopped",
            Self::EnsureProcessClosed => "ensure_process_closed",
            Self::RemoveHome => "remove_home",
            Self::RemoveWorktree => "remove_worktree",
            Self::PublishDeleted => "publish_deleted",
            Self::RestartConversation => "restart_conversation",
            Self::RestartTeardown => "restart_teardown",
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
    /// Immutable admission evidence; abandoned worktree data is retained until parent retirement.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worktree_recoveries: Vec<super::AgentWorktreeRecovery>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_skills: Vec<super::AgentSkillInstallation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_conversation_id: Option<String>,
    pub step: AgentWorkStep,
    pub ready_at: String,
    pub next_fence: u64,
    /// Commands through this order were admitted before the latest owner restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_restart_command_order: Option<u64>,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<AgentWorkLease>,
    #[serde(default)]
    pub receipts: Vec<AgentWorkReceipt>,
    #[serde(default)]
    pub cleanup_debt: Vec<AgentCleanupDebt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_not_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_skills: Option<Vec<ResolvedAgentSkill>>,
}

impl AgentWorkState {
    pub fn new(step: AgentWorkStep, ready_at: String) -> Self {
        Self {
            worktree_recoveries: Vec::new(),
            live_skills: Vec::new(),
            warm_conversation_id: None,
            step,
            ready_at,
            next_fence: 1,
            pre_restart_command_order: None,
            attempts: 0,
            lease: None,
            receipts: Vec::new(),
            cleanup_debt: Vec::new(),
            cleanup_not_before: None,
            resolved_skills: None,
        }
    }

    pub fn validate_resolved_skills(
        &self,
        references: &[SkillRef],
    ) -> Result<(), SandboxModelError> {
        if self.warm_conversation_id.as_ref().is_some_and(|id| {
            id.trim().is_empty()
                || id.len() > 512
                || id.trim() != id
                || id.chars().any(char::is_control)
        }) {
            return Err(SandboxModelError::invalid_value(
                "agent.work.warm_conversation_id",
            ));
        }
        super::agent_live_skills::validate_installations(self, references)?;
        let Some(material) = &self.resolved_skills else {
            return Ok(());
        };
        if material.len() > 32
            || material.len() != references.len()
            || !references.iter().zip(material).all(|(reference, skill)| {
                reference.source == skill.source
                    && reference.id == skill.id
                    && skill.validate().is_ok()
            })
        {
            return Err(SandboxModelError::invalid_value(
                "agent.work.resolved_skills",
            ));
        }
        Ok(())
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
        // These steps have no provider effects. After an interrupted publication,
        // a fresh claim can safely repeat the parent read or the local fence.
        let expires_at = if matches!(
            self.step,
            AgentWorkStep::WaitForWorkspace | AgentWorkStep::FenceInteraction
        ) {
            use std::time::{Duration, SystemTime};
            const PUBLICATION_ONLY_LEASE: Duration = Duration::from_secs(30);
            let claimed_at = crate::seconds_from_rfc3339(now)
                .ok_or_else(|| SandboxModelError::invalid_value("agent.work.lease"))?;
            let limit = crate::rfc3339_at(
                SystemTime::UNIX_EPOCH + Duration::from_secs(claimed_at) + PUBLICATION_ONLY_LEASE,
            );
            expires_at.min(limit)
        } else {
            expires_at
        };
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
        self.cleanup_not_before = None;
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
        let ready_at = match (self.step, next, self.cleanup_not_before.as_ref()) {
            (AgentWorkStep::FenceInteraction, AgentWorkStep::CloseProcess, Some(not_before)) => {
                // A delayed/restarted worker may fence after the old lease expires.
                // That satisfies the wait; it must not make cleanup permanently invalid.
                not_before.max(&ready_at).clone()
            }
            (_, _, Some(_)) => {
                return Err(SandboxModelError::invalid_value(
                    "agent.work.cleanup_not_before",
                ));
            }
            _ => ready_at,
        };
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
