use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    AgentWorkMutation, SandboxStoreError, VersionedWorktree, MAX_AGENT_WORK_RECEIPTS,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkReceipt, AgentWorkStep, Sandbox, SandboxEventKind,
    SandboxStatus, TypedFailure, Workspace, WorkspaceStatus, Worktree, WorktreeStatus,
};
use layer3_adapter::{AdapterError, RuntimeHandle};

use super::super::agent_skills::AgentSkillResolutionError;
use super::AgentWorker;

mod effects;
mod mapping;
pub(super) use crate::sandbox::runtime_mapping::{runtime_capabilities, runtime_options};
pub(super) use effects::{settle_cleanup_debt, settle_compensated_cleanup_debt};
pub(super) use mapping::{validate_runtime_skills, validate_skills};

pub(super) struct AgentContext {
    pub(super) sandbox: Sandbox,
    pub(super) workspace: Workspace,
    pub(super) worktree: Worktree,
    pub(super) runtime: RuntimeHandle,
}

#[derive(Debug, Clone)]
pub(super) struct RequestEvidence {
    pub(super) step: AgentWorkStep,
    pub(super) operation_id: String,
    pub(super) request_sha256: String,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{class}: {detail}")]
pub(in crate::sandbox) struct WorkerFailure {
    class: &'static str,
    detail: String,
    retryable: bool,
}

impl WorkerFailure {
    pub(super) fn retryable(class: &'static str, detail: impl Into<String>) -> Self {
        Self {
            class,
            detail: detail.into(),
            retryable: true,
        }
    }

    pub(super) fn terminal(class: &'static str, detail: impl Into<String>) -> Self {
        Self {
            class,
            detail: detail.into(),
            retryable: false,
        }
    }

    fn typed(&self) -> TypedFailure {
        TypedFailure {
            class: self.class.into(),
            detail: bounded(&self.detail, 4 * 1024),
        }
    }
}

impl AgentWorker {
    pub(super) async fn startup_context(
        &self,
        agent: &Agent,
    ) -> Result<AgentContext, WorkerFailure> {
        if agent.status != AgentStatus::Starting {
            return Err(WorkerFailure::terminal(
                "agent_lifecycle_state_invalid",
                "startup work was scheduled for a non-starting agent",
            ));
        }
        let context = self.context(agent).await?;
        if context.sandbox.status != SandboxStatus::Active
            || !context.sandbox.credential.delivered
            || context.workspace.status != WorkspaceStatus::Ready
            || context.worktree.status != WorktreeStatus::Creating
        {
            return Err(WorkerFailure::retryable(
                "agent_parent_not_ready",
                "the sandbox, workspace, or worktree is not ready for agent startup",
            ));
        }
        Ok(context)
    }

    pub(super) async fn cleanup_context(
        &self,
        agent: &Agent,
    ) -> Result<AgentContext, WorkerFailure> {
        self.context(agent).await
    }

    pub(super) async fn context(&self, agent: &Agent) -> Result<AgentContext, WorkerFailure> {
        // These IDs come from the claimed Agent. Read independently, then retain
        // the existing error precedence and validate all lineage before adoption.
        let (sandbox, workspace, worktree) = tokio::join!(
            self.dependencies.store.get_sandbox(&agent.sandbox_id),
            self.dependencies.store.get_workspace(&agent.workspace_id),
            self.dependencies.store.get_worktree(&agent.worktree_id),
        );
        let sandbox = sandbox
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        let workspace = workspace
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        let worktree = worktree
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        validate_lineage(agent, &sandbox, &workspace, &worktree)?;
        if sandbox.lifecycle.checkpoint_capture.is_some() {
            return Err(WorkerFailure::retryable(
                "agent_checkpoint_in_progress",
                "checkpoint writer resumption is pending",
            ));
        }
        let attached = crate::sandbox::runtime_attachment::adopt_bound_runtime(
            self.dependencies.adapter.as_ref(),
            &sandbox,
            &self.config.deployment_id,
        )
        .await
        .map_err(adapter_failure)?;
        Ok(AgentContext {
            sandbox,
            workspace,
            worktree,
            runtime: attached,
        })
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one atomic lifecycle publication carries its optional evidence and artifacts"
    )]
    pub(super) async fn progress(
        &self,
        mut agent: Agent,
        next_step: AgentWorkStep,
        now: SystemTime,
        evidence: Option<RequestEvidence>,
        mut logs: Vec<AgentLogKind>,
        worktree: Option<VersionedWorktree>,
        sandbox_event: Option<SandboxEventKind>,
    ) -> Result<(), SandboxStoreError> {
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let stamp = rfc3339_at(now);
        if let Some(evidence) = evidence {
            record_receipt(&mut agent, evidence, &stamp)?;
        }
        if logs.is_empty() {
            logs.push(AgentLogKind::WorkStepCompleted {
                step: agent.work.step,
            });
        }
        agent
            .work
            .advance(&self.config.holder, fence, next_step, stamp.clone())
            .map_err(|_| SandboxStoreError::Conflict)?;
        agent.updated_at = stamp.clone();
        agent.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
        bump_log_sequence(&mut agent, logs.len())?;
        self.dependencies
            .store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: expected,
                holder: self.config.holder.clone(),
                fence,
                agent,
                worktree,
                appended_at: stamp,
                sandbox_event,
                agent_logs: logs,
            })
            .await
    }

    pub(super) async fn handle_failure(
        &self,
        agent: Agent,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if failure.retryable || agent.status != AgentStatus::Starting {
            self.retry(agent, failure, now).await
        } else {
            self.fail_startup(agent, failure, now).await
        }
    }

    async fn retry(
        &self,
        mut agent: Agent,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let step = agent.work.step;
        let stamp = rfc3339_at(now);
        agent
            .work
            .retry(
                &self.config.holder,
                fence,
                rfc3339_at(now + self.config.retry_delay),
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        agent.updated_at = stamp.clone();
        agent.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
        bump_log_sequence(&mut agent, 1)?;
        self.dependencies
            .store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: expected,
                holder: self.config.holder.clone(),
                fence,
                agent,
                worktree: None,
                appended_at: stamp,
                sandbox_event: None,
                agent_logs: vec![AgentLogKind::WorkStepFailed {
                    step,
                    failure: failure.typed(),
                }],
            })
            .await
    }

    async fn fail_startup(
        &self,
        mut agent: Agent,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let step = agent.work.step;
        let stamp = rfc3339_at(now);
        let typed = failure.typed();
        let mut worktree = self
            .dependencies
            .store
            .get_worktree(&agent.worktree_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        let versioned = if worktree.status == WorktreeStatus::Creating {
            let expected_revision = worktree.revision;
            worktree
                .transition(WorktreeStatus::Failed)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            Some(VersionedWorktree {
                expected_revision,
                worktree,
            })
        } else {
            None
        };
        agent
            .work
            .advance(
                &self.config.holder,
                fence,
                AgentWorkStep::None,
                stamp.clone(),
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        agent
            .fail(typed.clone(), stamp.clone())
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        bump_log_sequence(&mut agent, 1)?;
        self.dependencies
            .registry
            .fence(&agent.id, agent.conversation.runtime_generation, fence);
        let id = agent.id.clone();
        self.dependencies
            .store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: expected,
                holder: self.config.holder.clone(),
                fence,
                agent,
                worktree: versioned,
                appended_at: stamp,
                sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                    agent_id: id,
                    from: AgentStatus::Starting,
                    to: AgentStatus::Failed,
                }),
                agent_logs: vec![AgentLogKind::WorkStepFailed {
                    step,
                    failure: typed,
                }],
            })
            .await
    }
}

pub(super) fn request_evidence(
    step: AgentWorkStep,
    operation_id: &str,
    request_sha256: &str,
) -> RequestEvidence {
    RequestEvidence {
        step,
        operation_id: operation_id.to_owned(),
        request_sha256: request_sha256.to_owned(),
    }
}

pub(super) fn claimed_fence(agent: &Agent, holder: &str) -> Result<u64, SandboxStoreError> {
    agent
        .work
        .lease
        .as_ref()
        .filter(|lease| lease.holder == holder)
        .map(|lease| lease.fence)
        .ok_or(SandboxStoreError::Conflict)
}

pub(super) fn bump_log_sequence(agent: &mut Agent, count: usize) -> Result<(), SandboxStoreError> {
    agent.conversation.last_seq = agent
        .conversation
        .last_seq
        .checked_add(u64::try_from(count).map_err(|_| SandboxStoreError::SequenceConflict)?)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    Ok(())
}

pub(super) fn adapter_failure(error: AdapterError) -> WorkerFailure {
    match error {
        AdapterError::Unavailable(detail) => {
            WorkerFailure::retryable("agent_runtime_unavailable", detail)
        }
        AdapterError::AtCapacity(detail) => WorkerFailure::terminal("agent_resource_quota", detail),
        AdapterError::Rejected(detail) | AdapterError::OperationIdentityConflict(detail) => {
            WorkerFailure::terminal("agent_runtime_rejected", detail)
        }
        AdapterError::Unsupported(detail) => {
            WorkerFailure::terminal("agent_runtime_unsupported", detail)
        }
        AdapterError::RuntimeContractMismatch(error) => {
            WorkerFailure::terminal("agent_runtime_contract_mismatch", error.to_string())
        }
    }
}

pub(super) fn skill_failure(error: AgentSkillResolutionError) -> WorkerFailure {
    match error {
        AgentSkillResolutionError::Unavailable(detail) => {
            WorkerFailure::retryable("agent_skill_source_unavailable", detail)
        }
        AgentSkillResolutionError::NotFound(detail) => {
            WorkerFailure::terminal("agent_skill_not_found", detail)
        }
        AgentSkillResolutionError::Invalid(detail) => {
            WorkerFailure::terminal("agent_skill_invalid", detail)
        }
    }
}

fn record_receipt(
    agent: &mut Agent,
    evidence: RequestEvidence,
    completed_at: &str,
) -> Result<(), SandboxStoreError> {
    if evidence.step != agent.work.step
        || evidence.operation_id.trim().is_empty()
        || evidence.operation_id.len() > 256
        || !valid_sha256(&evidence.request_sha256)
        || agent.work.receipts.len() >= MAX_AGENT_WORK_RECEIPTS
        || agent
            .work
            .receipts
            .iter()
            .any(|receipt| receipt.step == evidence.step)
    {
        return Err(SandboxStoreError::Corrupt(
            "agent lifecycle receipt history is invalid".into(),
        ));
    }
    settle_cleanup_debt(agent, &evidence)?;
    settle_compensated_cleanup_debt(agent, evidence.step);
    agent.work.receipts.push(AgentWorkReceipt {
        step: evidence.step,
        operation_id: evidence.operation_id,
        request_sha256: evidence.request_sha256,
        completed_at: completed_at.into(),
    });
    Ok(())
}

pub(super) fn validate_lineage(
    agent: &Agent,
    sandbox: &Sandbox,
    workspace: &Workspace,
    worktree: &Worktree,
) -> Result<(), WorkerFailure> {
    let valid = sandbox.id == agent.sandbox_id
        && sandbox.owner == agent.owner
        && workspace.id == agent.workspace_id
        && workspace.sandbox_id == sandbox.id
        && workspace.owner == agent.owner
        && worktree.id == agent.worktree_id
        && worktree.workspace_id == workspace.id
        && worktree.sandbox_id == sandbox.id
        && worktree.owner == agent.owner
        && worktree.agent_id.as_ref() == Some(&agent.id)
        && worktree.path == format!("{}/.worktrees/{}", workspace.path, worktree.name)
        && worktree.branch == format!("cma/{}", worktree.name)
        && workspace.resolved_commit.as_ref() == Some(&worktree.base_commit);
    if valid {
        Ok(())
    } else {
        Err(WorkerFailure::terminal(
            "agent_lineage_invalid",
            "agent parent records do not describe the same runtime resources",
        ))
    }
}

fn parent_store_failure(error: SandboxStoreError) -> WorkerFailure {
    match error {
        SandboxStoreError::Unavailable(detail) => {
            WorkerFailure::retryable("agent_store_unavailable", detail)
        }
        error => WorkerFailure::terminal("agent_parent_corrupt", error.to_string()),
    }
}

fn missing_parent() -> WorkerFailure {
    WorkerFailure::terminal("agent_parent_missing", "an agent parent record is missing")
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn bounded(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
