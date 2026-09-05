use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{AgentWorkMutation, SandboxStoreError, MAX_AGENT_CLEANUP_DEBT};
use layer2_core::sandbox::{Agent, AgentCleanupDebt, AgentWorkStep};

use super::{claimed_fence, RequestEvidence};
use crate::sandbox::agent_worker::AgentWorker;

impl AgentWorker {
    pub(in crate::sandbox::agent_worker) async fn prepare_side_effect(
        &self,
        mut agent: Agent,
        evidence: &RequestEvidence,
        now: SystemTime,
    ) -> Result<Agent, SandboxStoreError> {
        if evidence.step != agent.work.step {
            return Err(SandboxStoreError::Corrupt(
                "agent side-effect intent does not match its work step".into(),
            ));
        }
        if let Some(existing) = agent
            .work
            .cleanup_debt
            .iter()
            .find(|debt| debt.step == evidence.step)
        {
            return if existing.operation_id == evidence.operation_id
                && existing.request_sha256 == evidence.request_sha256
            {
                Ok(agent)
            } else {
                Err(SandboxStoreError::Corrupt(
                    "agent side-effect intent changed across retry".into(),
                ))
            };
        }
        if agent.work.cleanup_debt.len() >= MAX_AGENT_CLEANUP_DEBT {
            return Err(SandboxStoreError::Corrupt(
                "agent side-effect intent capacity exceeded".into(),
            ));
        }
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        agent.work.cleanup_debt.push(AgentCleanupDebt {
            step: evidence.step,
            operation_id: evidence.operation_id.clone(),
            request_sha256: evidence.request_sha256.clone(),
        });
        agent.updated_at = rfc3339_at(now);
        agent.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
        self.dependencies
            .store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: expected,
                holder: self.config.holder.clone(),
                fence,
                agent: agent.clone(),
                worktree: None,
                appended_at: rfc3339_at(now),
                sandbox_event: None,
                agent_logs: Vec::new(),
            })
            .await?;
        Ok(agent)
    }
}

pub(in crate::sandbox::agent_worker) fn settle_cleanup_debt(
    agent: &mut Agent,
    evidence: &RequestEvidence,
) -> Result<(), SandboxStoreError> {
    let Some(index) = agent
        .work
        .cleanup_debt
        .iter()
        .position(|debt| debt.step == evidence.step)
    else {
        return Ok(());
    };
    let debt = &agent.work.cleanup_debt[index];
    if debt.operation_id != evidence.operation_id || debt.request_sha256 != evidence.request_sha256
    {
        return Err(SandboxStoreError::Corrupt(
            "agent side-effect completion does not match its durable intent".into(),
        ));
    }
    agent.work.cleanup_debt.remove(index);
    Ok(())
}

pub(in crate::sandbox::agent_worker) fn settle_compensated_cleanup_debt(
    agent: &mut Agent,
    completed_step: AgentWorkStep,
) {
    let source_step = match completed_step {
        AgentWorkStep::CloseProcess | AgentWorkStep::EnsureProcessClosed => {
            AgentWorkStep::OpenConversation
        }
        AgentWorkStep::RemoveHome => AgentWorkStep::MaterializeHome,
        AgentWorkStep::RemoveWorktree => AgentWorkStep::AddWorktree,
        _ => return,
    };
    agent
        .work
        .cleanup_debt
        .retain(|debt| debt.step != source_step);
}
