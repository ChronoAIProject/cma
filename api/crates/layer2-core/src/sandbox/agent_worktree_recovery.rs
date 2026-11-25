//! Explicit recovery of a never-started Agent without touching indeterminate Git data.
use layer3_adapter::WorktreeRequest;
use serde::{Deserialize, Serialize};

use super::store::AgentControlStoreError;
use super::{
    Agent, AgentCleanupDebt, AgentOrigin, AgentStatus, AgentWorkStep, FirstMessageDelivery,
    Sandbox, SandboxModelError, SandboxStatus, SandboxWorkStep, TypedFailure, Workspace,
    WorkspaceStatus, WorkspaceWorkStep, Worktree, WorktreeStatus,
};
use crate::{Caller, Capability, SubjectId, WorktreeId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWorktreeRecovery {
    pub operation_id: String,
    pub requested_by: SubjectId,
    pub admitted_at: String,
    pub original_worktree: Worktree,
    pub original_failure: TypedFailure,
    pub original_cleanup_debt: AgentCleanupDebt,
    pub original_agent_revision: u64,
    pub original_log_sequence: u64,
    pub original_next_fence: u64,
    pub successor_worktree_id: WorktreeId,
}

pub struct AgentWorktreeRecoveryContext<'a> {
    pub agent: &'a Agent,
    pub worktree: &'a Worktree,
    pub workspace: &'a Workspace,
    pub sandbox: &'a Sandbox,
    pub caller: &'a Caller,
    pub live_agents: u8,
}

#[derive(Debug, Clone)]
pub struct AgentWorktreeRecoveryMutation {
    pub agent: Agent,
    pub retained_worktree: Worktree,
    pub successor_worktree: Worktree,
    pub workspace: Workspace,
}

fn valid_operation(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

fn is_indeterminate_checkout(failure: &TypedFailure) -> bool {
    failure.class == "agent_runtime_rejected"
        && failure
            .detail
            .starts_with("runtime_mutation_outcome_unknown: ")
}

fn exact_debt(debt: &AgentCleanupDebt, worktree: &Worktree, workspace_name: &str) -> bool {
    let request = WorktreeRequest::new(
        format!("worktree:{workspace_name}:{}:add:v1", worktree.name),
        workspace_name.to_owned(),
        worktree.name.clone(),
        worktree.base_commit.clone(),
    );
    debt.step == AgentWorkStep::AddWorktree
        && debt.operation_id == request.operation_id
        && debt.request_sha256 == request.request_sha256
}

impl Agent {
    /// Creation provenance and the initial log forever describe the original worktree.
    pub fn creation_worktree_id(&self) -> &WorktreeId {
        self.work
            .worktree_recoveries
            .first()
            .map_or(&self.worktree_id, |attempt| &attempt.original_worktree.id)
    }

    pub fn validate_worktree_recoveries(&self) -> Result<(), SandboxModelError> {
        let attempts = &self.work.worktree_recoveries;
        if attempts.len() > 64 {
            return Err(SandboxModelError::invalid_value(
                "agent.work.worktree_recoveries",
            ));
        }
        for (index, attempt) in attempts.iter().enumerate() {
            let original = &attempt.original_worktree;
            let workspace_name = original
                .path
                .strip_prefix(super::workspace::SANDBOX_HOME)
                .and_then(|path| path.strip_suffix(&format!("/.worktrees/{}", original.name)));
            let valid = valid_operation(&attempt.operation_id)
                && crate::seconds_from_rfc3339(&attempt.admitted_at).is_some()
                && !attempt.requested_by.as_str().is_empty()
                && original.workspace_id == self.workspace_id
                && original.sandbox_id == self.sandbox_id
                && original.owner == self.owner
                && original.agent_id.as_ref() == Some(&self.id)
                && original.status == WorktreeStatus::Failed
                && original.revision > 0
                && original.branch == format!("cma/{}", original.name)
                && !original.base_commit.is_empty()
                && original
                    .name
                    .strip_prefix("wt-")
                    .and_then(|v| v.parse::<u64>().ok())
                    .is_some_and(|v| v > 0)
                && is_indeterminate_checkout(&attempt.original_failure)
                && workspace_name.is_some_and(|name| {
                    !name.is_empty() && exact_debt(&attempt.original_cleanup_debt, original, name)
                })
                && attempt.original_agent_revision > 0
                && attempt.original_agent_revision < self.revision
                && attempt.original_log_sequence <= self.conversation.last_seq
                && attempt.original_next_fence > 0
                && attempt.original_next_fence < self.work.next_fence
                && attempt.successor_worktree_id != original.id
                && attempts[..index].iter().all(|previous| {
                    previous.operation_id != attempt.operation_id
                        && previous.original_worktree.id != original.id
                        && previous.original_worktree.id != attempt.successor_worktree_id
                })
                && (index == 0 || attempts[index - 1].successor_worktree_id == original.id)
                && (index + 1 != attempts.len()
                    || attempt.successor_worktree_id == self.worktree_id);
            if !valid {
                return Err(SandboxModelError::invalid_value(
                    "agent.work.worktree_recoveries",
                ));
            }
        }
        Ok(())
    }
}

/// Run only inside the store's transaction/lock. `None` is a durable idempotent
/// replay, including after a later delete; it must never resurrect that Agent.
pub fn plan_agent_worktree_recovery(
    context: AgentWorktreeRecoveryContext<'_>,
    operation_id: &str,
    successor_id: &WorktreeId,
    now: String,
) -> Result<Option<AgentWorktreeRecoveryMutation>, AgentControlStoreError> {
    use AgentControlStoreError as Error;
    let AgentWorktreeRecoveryContext {
        agent,
        worktree,
        workspace,
        sandbox,
        caller,
        live_agents,
    } = context;
    let admin = caller.authorization.allows(Capability::ControlAnySandbox);
    if !admin
        && (!caller.authorization.allows(Capability::UseOwnSandboxes)
            || caller.subject != agent.owner)
    {
        return Err(Error::NotFound);
    }
    agent
        .validate_worktree_recoveries()
        .map_err(|e| Error::Corrupt(e.to_string()))?;
    if !valid_operation(operation_id) || crate::seconds_from_rfc3339(&now).is_none() {
        return Err(Error::WorktreeRecoveryConflict);
    }
    if let Some(attempt) = agent
        .work
        .worktree_recoveries
        .iter()
        .find(|a| a.operation_id == operation_id)
    {
        return if attempt.requested_by == caller.subject {
            Ok(None)
        } else {
            Err(Error::WorktreeRecoveryConflict)
        };
    }
    let no_started_effects = agent.ready_at.is_none()
        && agent.conversation.conversation_id.is_none()
        && agent.conversation.runtime_generation.is_none()
        && agent.conversation.active_turn.is_none()
        && agent.conversation.operation.is_none()
        && agent.conversation.queued_messages.is_empty()
        && agent.conversation.observations.is_empty()
        && agent.work.receipts.is_empty()
        && agent.work.resolved_skills.is_none()
        && agent.work.warm_conversation_id.is_none()
        && agent.work.live_skills.is_empty()
        && agent
            .conversation
            .first_message
            .as_ref()
            .is_none_or(|message| {
                message.state == FirstMessageDelivery::Queued
                    && message.attempt == 0
                    && message.recovery_generation == 0
                    && message.fence_sequence == 0
                    && message.dispatch_generation == 0
                    && message.turn_sequence.is_none()
                    && message.dispatch_fenced_at.is_none()
                    && message.write_confirmed_at.is_none()
                    && message.execution_observed_at.is_none()
                    && message.completed_at.is_none()
                    && message.failed_at.is_none()
                    && message.failure.is_none()
                    && message.validate().is_ok()
            });
    if !no_started_effects
        || agent.status != AgentStatus::Failed
        || !matches!(
            agent.origin,
            AgentOrigin::Manual | AgentOrigin::WorkspaceProfile { .. }
        )
        || agent.work.step != AgentWorkStep::None
        || agent.work.lease.is_some()
        || agent.work.cleanup_not_before.is_some()
        || agent.requested_stop_reason.is_some()
        || agent.stop_reason.is_some()
        || agent.stopped_at.is_some()
        || agent.deleted_at.is_some()
        || agent.work.worktree_recoveries.len() >= 64
        || agent.work.cleanup_debt.len() != 1
        || agent
            .failure
            .as_ref()
            .is_none_or(|failure| !is_indeterminate_checkout(failure))
        || agent.id
            != *worktree
                .agent_id
                .as_ref()
                .ok_or(Error::WorktreeRecoveryConflict)?
        || agent.worktree_id != worktree.id
        || agent.workspace_id != workspace.id
        || agent.sandbox_id != sandbox.id
        || agent.owner != sandbox.owner
        || worktree.status != WorktreeStatus::Failed
        || workspace.status != WorkspaceStatus::Ready
        || workspace.resolved_commit.is_none()
        || workspace.work.step != WorkspaceWorkStep::None
        || workspace.work.lease.is_some()
        || workspace.removed_at.is_some()
        || workspace.owner != agent.owner
        || workspace.sandbox_id != sandbox.id
        || sandbox.status != SandboxStatus::Active
        || !sandbox.credential.delivered
        || sandbox.lifecycle.operation.is_some()
        || sandbox.lifecycle.checkpoint_capture.is_some()
        || sandbox.lifecycle.runtime_provision.is_some()
        || sandbox.lifecycle.home_recovery.is_some()
        || sandbox.lifecycle.unpublished_recovery.is_some()
        || sandbox.lifecycle.failed_provision.is_some()
        || sandbox.work.lease.is_some()
        || !matches!(
            sandbox.work.step,
            SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
        )
    {
        return Err(Error::WorktreeRecoveryConflict);
    }
    worktree
        .validate_prerequisites(workspace)
        .map_err(|e| Error::Corrupt(e.to_string()))?;
    if !exact_debt(&agent.work.cleanup_debt[0], worktree, &workspace.name)
        || successor_id == &worktree.id
        || agent
            .work
            .worktree_recoveries
            .iter()
            .any(|a| &a.original_worktree.id == successor_id)
        || workspace.next_worktree_number
            <= worktree.name[3..]
                .parse::<u64>()
                .map_err(|_| Error::WorktreeRecoveryConflict)?
    {
        return Err(Error::WorktreeRecoveryConflict);
    }
    if live_agents >= sandbox.limits.max_agents_per_workspace {
        return Err(Error::LimitReached {
            limit: sandbox.limits.max_agents_per_workspace,
            live: live_agents,
        });
    }
    let mut next_workspace = workspace.clone();
    let name = format!("wt-{}", workspace.next_worktree_number);
    next_workspace.next_worktree_number = workspace
        .next_worktree_number
        .checked_add(1)
        .ok_or(Error::Conflict)?;
    next_workspace.revision = workspace.revision.checked_add(1).ok_or(Error::Conflict)?;
    let successor = Worktree {
        id: successor_id.clone(),
        workspace_id: workspace.id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: agent.owner.clone(),
        path: format!("{}/.worktrees/{name}", workspace.path),
        branch: format!("cma/{name}"),
        name,
        base_commit: worktree.base_commit.clone(),
        status: WorktreeStatus::Creating,
        agent_id: Some(agent.id.clone()),
        revision: 1,
    };
    successor
        .validate_prerequisites(workspace)
        .map_err(|e| Error::Corrupt(e.to_string()))?;
    let mut next = agent.clone();
    next.work.worktree_recoveries.push(AgentWorktreeRecovery {
        operation_id: operation_id.into(),
        requested_by: caller.subject.clone(),
        admitted_at: now.clone(),
        original_worktree: worktree.clone(),
        original_failure: agent.failure.clone().expect("validated failure"),
        original_cleanup_debt: agent.work.cleanup_debt[0].clone(),
        original_agent_revision: agent.revision,
        original_log_sequence: agent.conversation.last_seq,
        original_next_fence: agent.work.next_fence,
        successor_worktree_id: successor.id.clone(),
    });
    next.worktree_id = successor.id.clone();
    next.status = AgentStatus::Starting;
    next.failure = None;
    next.updated_at = now.clone();
    next.revision = agent.revision.checked_add(1).ok_or(Error::Conflict)?;
    next.work.next_fence = agent
        .work
        .next_fence
        .checked_add(1)
        .ok_or(Error::Conflict)?;
    next.work.cleanup_debt.clear(); // Exact original debt remains in the immutable archive above.
    next.work.schedule(AgentWorkStep::AddWorktree, now);
    next.validate_worktree_recoveries()
        .map_err(|e| Error::Corrupt(e.to_string()))?;
    let mut retained_worktree = worktree.clone();
    retained_worktree.agent_id = None;
    retained_worktree.revision = worktree.revision.checked_add(1).ok_or(Error::Conflict)?;
    Ok(Some(AgentWorktreeRecoveryMutation {
        agent: next,
        retained_worktree,
        successor_worktree: successor,
        workspace: next_workspace,
    }))
}
