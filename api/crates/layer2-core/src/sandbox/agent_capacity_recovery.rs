//! Read-only capacity recovery preflight. This does not authorize adoption.
//!
//! Callers must authorize access and capture all four records coherently. A
//! candidate still needs validated source history, exact effect receipts, both
//! durable gates and write fences in the segment writer's admission transaction.

use serde::{Deserialize, Serialize};

use super::store::AGENT_LOG_CAPACITY_FAILURE;
use super::{
    Agent, AgentLogEntry, AgentLogKind, AgentStatus, AgentWorkStep, RuntimeBinding, Sandbox,
    SandboxStatus, SandboxWorkStep, Workspace, WorkspaceStatus, WorkspaceWorkStep, Worktree,
    WorktreeStatus,
};
use crate::seconds_from_rfc3339;

pub mod activation;
pub mod commands;
pub mod ports;
pub mod preflight;
pub mod service;
pub mod state;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityRecoveryDisposition {
    /// Metadata permits bounded evidence inspection, not an admission or Ready.
    EvidenceRequired,
    /// Candidate for the original publication, subject to writer capacity and
    /// receipt proof. Never replace its operation/effect identities.
    ContinueOriginalWork,
    ParentRecoveryRequired,
    ParentNotReady,
    CleanupOnly,
    TerminalPreserved,
    LeaseActive,
    NotCapacityFailure,
    LineageMismatch,
    InvalidState,
    NativeIdentityRequired,
    HomeIdentityRequired,
    OpenReceiptRequired,
    EffectReconciliationRequired,
    FailureEvidenceMismatch,
    SourceChanged,
}

pub struct CapacityRecoveryContext<'a> {
    pub agent: &'a Agent,
    pub sandbox: &'a Sandbox,
    pub workspace: &'a Workspace,
    pub worktree: &'a Worktree,
}

/// A transient domain proposal, not a durable receipt or a source-integrity
/// certificate. The writer must validate the captured prefix/format/physical
/// limits and persist this proposal with gates, successor and recovery work in
/// one transaction. In particular, persisting only `fenced_agent` is invalid.
#[derive(Debug, Clone)]
pub struct CapacityRecoveryPlan {
    source: Agent,
    original_failure: AgentLogEntry,
    sandbox_revision: u64,
    workspace_revision: u64,
    worktree_revision: u64,
    runtime: RuntimeBinding,
    home_identity: String,
    admitted_at: String,
}

impl CapacityRecoveryPlan {
    pub fn source(&self) -> &Agent {
        &self.source
    }

    pub fn original_failure(&self) -> &AgentLogEntry {
        &self.original_failure
    }

    pub fn admitted_at(&self) -> &str {
        &self.admitted_at
    }

    /// Fresh time checks expired leases; the admitted proposal and timestamp
    /// remain identical across known-aborted retries. Unknown commit requires
    /// the writer's exact receipt reconciliation before planning another write.
    pub fn fenced_agent(
        &self,
        context: &CapacityRecoveryContext<'_>,
        now: &str,
    ) -> Result<Agent, CapacityRecoveryDisposition> {
        use CapacityRecoveryDisposition as D;
        let disposition = context.classify(now);
        if disposition != D::EvidenceRequired {
            return Err(disposition);
        }
        if context.agent != &self.source
            || context.sandbox.revision != self.sandbox_revision
            || context.workspace.revision != self.workspace_revision
            || context.worktree.revision != self.worktree_revision
            || context.sandbox.runtime.as_ref() != Some(&self.runtime)
            || home_identity(context.sandbox).as_deref() != Some(&self.home_identity)
            || seconds_from_rfc3339(now) < seconds_from_rfc3339(&self.admitted_at)
        {
            return Err(D::SourceChanged);
        }
        let mut next = self.source.clone();
        next.revision = next.revision.checked_add(1).ok_or(D::InvalidState)?;
        next.work.next_fence = next.work.next_fence.checked_add(1).ok_or(D::InvalidState)?;
        next.work.lease = None;
        next.updated_at = self.admitted_at.clone();
        // Do not schedule PublishConnection or DispatchFirstMessage here.
        // S4's atomic admission carries separate non-ready recovery work.
        Ok(next)
    }
}

impl CapacityRecoveryContext<'_> {
    /// Build from the exact original failure row in the caller's validated
    /// captured prefix. Point-row shape alone does not prove source integrity.
    pub fn plan(
        &self,
        original_failure: &AgentLogEntry,
        at: &str,
    ) -> Result<CapacityRecoveryPlan, CapacityRecoveryDisposition> {
        use CapacityRecoveryDisposition as D;
        let disposition = self.classify(at);
        if disposition != D::EvidenceRequired {
            return Err(disposition);
        }
        let agent = self.agent;
        if original_failure.agent_id != agent.id
            || original_failure.seq != agent.conversation.last_seq
            || original_failure.seq < 2
            || !matches!(&original_failure.kind,
                AgentLogKind::WorkStepFailed { step, failure }
                if Some(failure) == agent.failure.as_ref()
                    && matches!(step, AgentWorkStep::InstallSkill | AgentWorkStep::WaitForWorkspace
                        | AgentWorkStep::AddWorktree | AgentWorkStep::MaterializeHome
                        | AgentWorkStep::OpenConversation | AgentWorkStep::PublishReady
                        | AgentWorkStep::PublishConnection | AgentWorkStep::DispatchFirstMessage))
            || seconds_from_rfc3339(&original_failure.appended_at).is_none_or(|stamp| {
                Some(stamp) < seconds_from_rfc3339(&agent.created_at)
                    || Some(stamp) > seconds_from_rfc3339(&agent.updated_at)
            })
        {
            return Err(D::FailureEvidenceMismatch);
        }
        Ok(CapacityRecoveryPlan {
            source: agent.clone(),
            original_failure: original_failure.clone(),
            sandbox_revision: self.sandbox.revision,
            workspace_revision: self.workspace.revision,
            worktree_revision: self.worktree.revision,
            runtime: self
                .sandbox
                .runtime
                .clone()
                .ok_or(D::NativeIdentityRequired)?,
            home_identity: home_identity(self.sandbox).ok_or(D::HomeIdentityRequired)?,
            admitted_at: at.into(),
        })
    }

    pub fn classify(&self, now: &str) -> CapacityRecoveryDisposition {
        use CapacityRecoveryDisposition as D;
        let Self {
            agent,
            sandbox,
            workspace,
            worktree,
        } = self;
        // Check lineage before returning any parent-derived disposition.
        if !self.same_lineage() {
            return D::LineageMismatch;
        }
        if matches!(agent.status, AgentStatus::Stopped | AgentStatus::Deleted) {
            return D::TerminalPreserved;
        }
        if agent.requested_stop_reason.is_some()
            || agent.stop_reason.is_some()
            || agent.stopped_at.is_some()
            || agent.deleted_at.is_some()
            || agent.work.cleanup_not_before.is_some()
            || cleanup_step(agent.work.step)
            || matches!(
                sandbox.status,
                SandboxStatus::Retiring | SandboxStatus::Retired
            )
            || sandbox.retired_at.is_some()
            || matches!(
                workspace.status,
                WorkspaceStatus::Removing | WorkspaceStatus::Removed
            )
            || workspace.removed_at.is_some()
            || matches!(
                worktree.status,
                WorktreeStatus::Removing | WorktreeStatus::Removed
            )
        {
            return D::CleanupOnly;
        }
        let failed = agent.status == AgentStatus::Failed
            && agent
                .failure
                .as_ref()
                .is_some_and(|f| f.class == AGENT_LOG_CAPACITY_FAILURE)
            && agent.work.step == AgentWorkStep::None;
        let pending = agent.failure.is_none()
            && matches!(
                (agent.status, agent.work.step),
                (AgentStatus::Starting, AgentWorkStep::PublishReady)
                    | (
                        AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval,
                        AgentWorkStep::PublishConnection
                    )
            );
        if !failed && !pending {
            return D::NotCapacityFailure;
        }
        let Some(now) = seconds_from_rfc3339(now) else {
            return D::InvalidState;
        };
        if agent.revision == 0
            || agent.revision == u64::MAX
            || agent.work.next_fence == 0
            || agent.work.next_fence == u64::MAX
            || agent.conversation.last_seq == 0
            || agent.conversation.last_seq == u64::MAX
            || sandbox.revision == 0
            || workspace.revision == 0
            || worktree.revision == 0
            || agent.creation_key.trim().is_empty()
            || agent.creation_key.len() > 256
            || !valid_sha256(&agent.creation_fingerprint)
            || [
                agent.created_at.as_str(),
                agent.updated_at.as_str(),
                agent.work.ready_at.as_str(),
                sandbox.updated_at.as_str(),
                workspace.created_at.as_str(),
            ]
            .iter()
            .any(|at| seconds_from_rfc3339(at).is_none_or(|at| at > now))
            || agent.validate_conversation().is_err()
        {
            return D::InvalidState;
        }
        if sandbox.status == SandboxStatus::Failed
            || workspace.status == WorkspaceStatus::Failed
            || worktree.status == WorktreeStatus::Failed
        {
            return D::ParentRecoveryRequired;
        }
        if sandbox.status != SandboxStatus::Active
            || workspace.status != WorkspaceStatus::Ready
            || worktree.status != WorktreeStatus::Ready
            || !sandbox.credential.delivered
            || sandbox.lifecycle.operation.is_some()
            || sandbox.lifecycle.checkpoint_capture.is_some()
            || sandbox.lifecycle.runtime_provision.is_some()
            || sandbox.lifecycle.home_recovery.is_some()
            || sandbox.lifecycle.unpublished_recovery.is_some()
            || sandbox.lifecycle.failed_provision.is_some()
            || !matches!(
                sandbox.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
            || workspace.work.step != WorkspaceWorkStep::None
        {
            return D::ParentNotReady;
        }
        // Invalid leases are corruption, never evidence that authority expired.
        if let Some(lease) = &agent.work.lease {
            if lease.holder.trim().is_empty()
                || lease.fence == 0
                || lease.fence >= agent.work.next_fence
            {
                return D::InvalidState;
            }
            match seconds_from_rfc3339(&lease.expires_at) {
                None => return D::InvalidState,
                Some(expires) if now < expires => return D::LeaseActive,
                Some(_) => {}
            }
        }
        // A parent lease is settled by its owner, even when its time has passed.
        if sandbox.work.lease.is_some() || workspace.work.lease.is_some() {
            return D::ParentNotReady;
        }
        if agent.validate_prerequisites(worktree).is_err()
            || worktree.validate_prerequisites(workspace).is_err()
            || workspace.validate_prerequisites(sandbox).is_err()
            || workspace.resolved_commit.as_ref() != Some(&worktree.base_commit)
        {
            return D::InvalidState;
        }
        if agent.conversation.conversation_id.is_none()
            || agent.conversation.runtime_generation.is_none()
            || sandbox.runtime.as_ref().is_none_or(|runtime| {
                runtime.runtime_id.trim().is_empty()
                    || runtime.runtime_version.trim().is_empty()
                    || runtime
                        .boot_id
                        .as_ref()
                        .is_none_or(|id| id.trim().is_empty())
                    // Provider allocation and native conversation generations
                    // are independent counters; pin both, never equate them.
                    || runtime.provision_generation.is_none_or(|generation| generation == 0)
            })
        {
            return D::NativeIdentityRequired;
        }
        if sandbox.lifecycle.home.as_ref().is_none_or(|home| {
            home.validate(&sandbox.id).is_err()
                || home
                    .native_pool
                    .as_ref()
                    .is_some_and(|native| native.cleaned || native.cleanup.is_some())
                || home
                    .attachment()
                    .is_none_or(|attachment| attachment.identity().is_err())
        }) {
            return D::HomeIdentityRequired;
        }
        if sandbox.validate().is_err() {
            return D::InvalidState;
        }
        let mut open = agent
            .work
            .receipts
            .iter()
            .filter(|r| r.step == AgentWorkStep::OpenConversation);
        if open.next().is_none_or(|receipt| {
            receipt.operation_id != format!("agent:{}:open:v1", agent.id)
                || !valid_sha256(&receipt.request_sha256)
                || seconds_from_rfc3339(&receipt.completed_at).is_none_or(|at| at > now)
        }) || open.next().is_some()
        {
            return D::OpenReceiptRequired;
        }
        if !agent.work.cleanup_debt.is_empty() {
            return D::EffectReconciliationRequired;
        }
        if pending {
            D::ContinueOriginalWork
        } else {
            D::EvidenceRequired
        }
    }

    fn same_lineage(&self) -> bool {
        let Self {
            agent,
            sandbox,
            workspace,
            worktree,
        } = self;
        agent.sandbox_id == sandbox.id
            && agent.owner == sandbox.owner
            && agent.workspace_id == workspace.id
            && workspace.sandbox_id == sandbox.id
            && workspace.owner == agent.owner
            && agent.worktree_id == worktree.id
            && worktree.workspace_id == workspace.id
            && worktree.sandbox_id == sandbox.id
            && worktree.owner == agent.owner
            && worktree.agent_id.as_ref() == Some(&agent.id)
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn home_identity(sandbox: &Sandbox) -> Option<String> {
    sandbox
        .lifecycle
        .home
        .as_ref()?
        .attachment()?
        .identity()
        .ok()
}

fn cleanup_step(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::FenceInteraction
            | AgentWorkStep::CloseProcess
            | AgentWorkStep::PublishStopped
            | AgentWorkStep::EnsureProcessClosed
            | AgentWorkStep::RemoveHome
            | AgentWorkStep::RemoveWorktree
            | AgentWorkStep::PublishDeleted
    )
}

#[cfg(test)]
mod tests;
