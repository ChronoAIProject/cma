use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    AgentRetryObservation, AgentWorkMutation, AgentWorkPublicationLineage,
    AgentWorkPublicationParents, SandboxStoreError, VersionedWorktree, MAX_AGENT_WORK_RECEIPTS,
};
use layer2_core::sandbox::work_retry::WorkRetryFailure;
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkReceipt, AgentWorkStep, Sandbox, SandboxEventKind,
    TypedFailure, Workspace, Worktree, WorktreeStatus,
};
use layer3_adapter::{AdapterError, RuntimeHandle};

use super::super::agent_skills::AgentSkillResolutionError;
use super::AgentWorker;

mod context;
mod effects;
mod failures;
mod lifecycle_failure;
pub(super) use failures::{adapter_failure, cleanup_failure, skill_failure};
use failures::{missing_parent, parent_store_failure};
#[cfg(test)]
mod ledger_hold_tests;
mod mapping;
mod publication;
pub(super) use publication::{same_publication_claim, PublicationParents};
mod repeated_failure;
mod retry_delay;
#[cfg(test)]
mod startup_hold_tests;
pub(super) use crate::sandbox::runtime_mapping::{runtime_capabilities, runtime_options};
pub(super) use effects::{settle_cleanup_debt, settle_compensated_cleanup_debt};
pub(super) use mapping::{validate_runtime_skills, validate_skills};

pub(super) struct AgentContext {
    pub(super) sandbox: Sandbox,
    pub(super) workspace: Workspace,
    pub(super) worktree: Worktree,
    pub(super) runtime: RuntimeHandle,
    pub(super) agent_resource_preparation: bool,
}

impl AgentContext {
    pub(super) fn publication_parents(&self) -> AgentWorkPublicationParents {
        AgentWorkPublicationParents {
            sandbox_revision: self.sandbox.revision,
            workspace_revision: self.workspace.revision,
            worktree_revision: self.worktree.revision,
            lineage: AgentWorkPublicationLineage::Authorized,
        }
    }
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
    observation: Option<AgentRetryObservation>,
    rejected_lineage: Option<Box<AgentWorkPublicationParents>>,
    retry_kind: WorkRetryFailure,
}

impl WorkerFailure {
    pub(in crate::sandbox) fn retryable(class: &'static str, detail: impl Into<String>) -> Self {
        Self {
            class,
            detail: detail.into(),
            retryable: true,
            observation: None,
            rejected_lineage: None,
            retry_kind: if class == "agent_parent_not_ready" {
                WorkRetryFailure::Waiting
            } else {
                WorkRetryFailure::Transient
            },
        }
    }

    pub(super) fn terminal(class: &'static str, detail: impl Into<String>) -> Self {
        Self {
            class,
            detail: detail.into(),
            retryable: false,
            observation: None,
            rejected_lineage: None,
            retry_kind: WorkRetryFailure::ExternalAction,
        }
    }

    pub(in crate::sandbox::agent_worker) fn is_ledger_hold(&self) -> bool {
        self.class == layer2_core::sandbox::store::AGENT_RUNTIME_LEDGER_HOLD
    }

    pub(in crate::sandbox::agent_worker) fn is_startup_hold(&self) -> bool {
        self.class == layer2_core::sandbox::store::AGENT_RUNTIME_STARTUP_HOLD
    }

    pub(in crate::sandbox::agent_worker) fn is_runtime_hold(&self) -> bool {
        self.is_ledger_hold() || self.is_startup_hold()
    }

    fn typed(&self) -> TypedFailure {
        TypedFailure {
            class: self.class.into(),
            detail: bounded(&self.detail, 4 * 1024),
        }
    }
}

impl AgentWorker {
    #[allow(
        clippy::too_many_arguments,
        reason = "one atomic lifecycle publication carries its optional evidence and artifacts"
    )]
    pub(super) async fn progress(
        &self,
        agent: Agent,
        next_step: AgentWorkStep,
        now: SystemTime,
        evidence: Option<RequestEvidence>,
        logs: Vec<AgentLogKind>,
        worktree: Option<VersionedWorktree>,
        sandbox_event: Option<SandboxEventKind>,
    ) -> Result<(), SandboxStoreError> {
        let mutation = self.progress_mutation(
            agent,
            next_step,
            now,
            evidence,
            logs,
            worktree,
            sandbox_event,
        )?;
        self.dependencies.store.commit_agent_work(mutation).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn progress_mutation(
        &self,
        mut agent: Agent,
        next_step: AgentWorkStep,
        now: SystemTime,
        evidence: Option<RequestEvidence>,
        mut logs: Vec<AgentLogKind>,
        worktree: Option<VersionedWorktree>,
        sandbox_event: Option<SandboxEventKind>,
    ) -> Result<AgentWorkMutation, SandboxStoreError> {
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
        Ok(AgentWorkMutation {
            expected_agent_revision: expected,
            holder: self.config.holder.clone(),
            fence,
            agent,
            worktree,
            appended_at: stamp,
            sandbox_event,
            agent_logs: logs,
        })
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

pub(super) fn record_receipt(
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
        && (workspace.resolved_commit.as_ref() == Some(&worktree.base_commit)
            || (worktree.base_commit.is_empty()
                && agent.has_no_runtime_effects()
                && matches!((&agent.origin, &sandbox.origin),
                    (layer2_core::sandbox::AgentOrigin::Lifecycle { operation_id: a },
                    layer2_core::sandbox::SandboxOrigin::Lifecycle { operation_id: s }) if a == s)));
    if valid {
        Ok(())
    } else {
        let mut failure = WorkerFailure::terminal(
            "agent_lineage_invalid",
            "agent parent records do not describe the same runtime resources",
        );
        if sandbox.id == agent.sandbox_id
            && sandbox.owner == agent.owner
            && workspace.id == agent.workspace_id
            && worktree.id == agent.worktree_id
            && worktree.workspace_id == agent.workspace_id
            && worktree.sandbox_id == agent.sandbox_id
            && worktree.owner == agent.owner
            && worktree.agent_id.as_ref() == Some(&agent.id)
        {
            failure.rejected_lineage = Some(Box::new(AgentWorkPublicationParents {
                sandbox_revision: sandbox.revision,
                workspace_revision: workspace.revision,
                worktree_revision: worktree.revision,
                lineage: AgentWorkPublicationLineage::Rejected {
                    workspace_foreign: workspace.owner != agent.owner,
                },
            }));
        }
        Err(failure)
    }
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
