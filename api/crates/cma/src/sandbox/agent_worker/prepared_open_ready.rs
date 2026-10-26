//! Complete an actually opened prepared conversation and publish Ready in one commit.
use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    AgentWorkMutation, PreparedAgentOpenContext, SandboxStoreError, VersionedWorktree,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkStep, SandboxEventKind, WorktreeStatus,
};

use super::support::{
    bump_log_sequence, claimed_fence, record_receipt, AgentContext, RequestEvidence,
};
use super::AgentWorker;
use crate::sandbox::live_agents::{LiveAgentAttachment, LiveAgentTarget};

mod connection;
mod recovery;

impl AgentWorker {
    pub(super) async fn publish_prepared_open_ready(
        &self,
        agent: &Agent,
        context: &AgentContext,
        evidence: &RequestEvidence,
        attachment: &LiveAgentAttachment,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let identity = attachment.identity();
        if agent.work.warm_conversation_id.as_deref() != Some(identity.conversation_id.as_str())
            || agent.conversation.settings.is_some()
            || !agent.profile.content.skills.is_empty()
            || !crate::agent_lifecycle::guidance::is_prepared_prompt(
                &agent.profile.content.system_prompt,
            )
            || agent.work.resolved_skills.as_deref() != Some(&[])
        {
            return Ok(false);
        }
        let observed = PreparedAgentOpenContext {
            sandbox: context.sandbox.clone(),
            workspace: context.workspace.clone(),
            worktree: context.worktree.clone(),
        };
        self.retry_startup_publication(agent.clone(), &context.sandbox, now, |current, at| {
            let observed = observed.clone();
            let evidence = evidence.clone();
            let identity = identity.clone();
            async move {
                let fence = claimed_fence(&current, &self.config.holder)?;
                let target = LiveAgentTarget {
                    agent_id: current.id.clone(),
                    conversation_id: identity.conversation_id.clone(),
                    runtime_generation: identity.runtime_generation,
                };
                let staged = self
                    .dependencies
                    .registry
                    .staged_attachment(&target, fence)
                    .map_err(|_| SandboxStoreError::Conflict)?;
                if staged
                    .as_ref()
                    .is_none_or(|staged| staged.identity() != identity)
                {
                    return Err(SandboxStoreError::Conflict);
                }
                let mutation =
                    prepared_ready_mutation(current, &observed, evidence, &self.config.holder, at)?;
                self.dependencies
                    .store
                    .commit_prepared_agent_open_ready(mutation, observed)
                    .await
            }
        })
        .await
    }
}

fn prepared_ready_mutation(
    mut agent: Agent,
    observed: &PreparedAgentOpenContext,
    evidence: RequestEvidence,
    holder: &str,
    now: SystemTime,
) -> Result<AgentWorkMutation, SandboxStoreError> {
    let expected = agent.revision;
    let fence = claimed_fence(&agent, holder)?;
    let stamp = rfc3339_at(now);
    let conversation_id = agent
        .work
        .warm_conversation_id
        .clone()
        .ok_or(SandboxStoreError::Conflict)?;
    agent.conversation.conversation_id = Some(conversation_id.clone());
    agent.conversation.runtime_generation = Some(1);
    record_receipt(&mut agent, evidence, &stamp)?;
    agent
        .work
        .advance(
            holder,
            fence,
            AgentWorkStep::PublishConnection,
            stamp.clone(),
        )
        .map_err(|_| SandboxStoreError::Conflict)?;
    agent
        .transition(AgentStatus::Ready, stamp.clone(), None)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let mut tree = observed.worktree.clone();
    tree.transition(WorktreeStatus::Ready)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let logs = vec![
        AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::OpenConversation,
        },
        AgentLogKind::ConversationOpened {
            conversation_id,
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    bump_log_sequence(&mut agent, logs.len())?;
    Ok(AgentWorkMutation {
        expected_agent_revision: expected,
        holder: holder.into(),
        fence,
        worktree: Some(VersionedWorktree {
            expected_revision: observed.worktree.revision,
            worktree: tree,
        }),
        appended_at: stamp,
        sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
            agent_id: agent.id.clone(),
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        }),
        agent,
        agent_logs: logs,
    })
}

#[cfg(test)]
mod tests;
