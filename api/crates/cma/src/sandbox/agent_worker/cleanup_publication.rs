use super::{
    support::{bump_log_sequence, claimed_fence},
    AgentWorker,
};
use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    AgentWorkMutation, AgentWorkPublication, AgentWorkPublicationParents, SandboxStoreError,
    VersionedWorktree,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentStopReason, AgentWorkStep, SandboxEventKind,
    SandboxStatus, WorkspaceStatus, WorktreeStatus,
};
use std::time::SystemTime;

impl AgentWorker {
    async fn prove_owner_stop(&self, agent: &Agent) -> Option<AgentWorkPublicationParents> {
        if !self.config.owner_stop_settlement_enabled
            || agent.requested_stop_reason != Some(AgentStopReason::Owner)
            || agent.conversation.active_turn.is_none()
        {
            return None;
        }
        if !self
            .dependencies
            .store
            .has_owner_stop_receipt(agent)
            .await
            .ok()?
        {
            return None;
        }
        let native = self.dependencies.adapter.runtime_recovery()?;
        let opened = agent
            .work
            .receipts
            .iter()
            .find(|receipt| receipt.step == AgentWorkStep::OpenConversation)?;
        let closed = agent
            .work
            .receipts
            .iter()
            .find(|receipt| receipt.step == AgentWorkStep::CloseProcess)?;
        let context = self.cleanup_context(agent).await.ok()?;
        if context.sandbox.status != SandboxStatus::Active
            || context.sandbox.lifecycle.operation.is_some()
            || context.workspace.status != WorkspaceStatus::Ready
            || context.worktree.status != WorktreeStatus::Ready
        {
            return None;
        }
        let status = self
            .dependencies
            .adapter
            .runtime_status(&context.runtime)
            .await
            .ok()?;
        let binding = context.sandbox.runtime.as_ref()?;
        if binding.boot_id.as_ref() != Some(&status.boot_id)
            || !context.sandbox.runtime_process_matches(&status).ok()?
        {
            return None;
        }
        let proof = layer3_adapter::wire::AgentProcessCloseProof {
            runtime: status.restart_runtime().ok()?,
            close: layer3_adapter::AgentCloseRequest {
                operation_id: closed.operation_id.clone(),
                request_sha256: closed.request_sha256.clone(),
                agent_id: layer3_adapter::AgentSlot::new(agent.id.as_str()),
            },
            open_request_sha256: opened.request_sha256.clone(),
            worktree_path: context.worktree.path.clone(),
        };
        native
            .prove_agent_process_closed(&context.runtime, &proof)
            .await
            .ok()?;
        Some(context.publication_parents())
    }

    pub(super) async fn publish_stopped(
        &self,
        mut agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(reason) = agent.requested_stop_reason.clone() else {
            return self.invalid_cleanup(agent, now).await;
        };
        if !agent.status.is_live() {
            return self.invalid_cleanup(agent, now).await;
        }
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let started = tokio::time::Instant::now();
        let previous = agent.status;
        let mut worktree = self
            .dependencies
            .store
            .get_worktree(&agent.worktree_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        let settlement = if worktree.status == WorktreeStatus::Ready {
            self.prove_owner_stop(&agent).await.and_then(|parents| {
                let mut settled = agent.clone();
                let event = settled.settle_owner_stop_turn()?;
                Some((parents, settled.conversation, event))
            })
        } else {
            None
        };
        let stamp = if settlement.is_some() {
            rfc3339_at(now + started.elapsed())
        } else {
            rfc3339_at(now)
        };
        let versioned = match worktree.status {
            WorktreeStatus::Creating => {
                let expected_revision = worktree.revision;
                worktree
                    .transition(WorktreeStatus::Failed)
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
                Some(VersionedWorktree {
                    expected_revision,
                    worktree,
                })
            }
            WorktreeStatus::Ready | WorktreeStatus::Failed => None,
            WorktreeStatus::Removing | WorktreeStatus::Removed => {
                return self.invalid_cleanup(agent, now).await;
            }
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
            .transition(AgentStatus::Stopped, stamp.clone(), Some(reason.clone()))
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        bump_log_sequence(&mut agent, 1)?;
        self.dependencies
            .registry
            .fence(&agent.id, agent.conversation.runtime_generation, fence);
        let id = agent.id.clone();
        let mutation = AgentWorkMutation {
            expected_agent_revision: expected,
            holder: self.config.holder.clone(),
            fence,
            agent,
            worktree: versioned,
            appended_at: stamp,
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: id,
                from: previous,
                to: AgentStatus::Stopped,
            }),
            agent_logs: vec![AgentLogKind::Stopped {
                reason: reason.as_str().into(),
            }],
        };
        let store = &self.dependencies.store;
        if let Some((parents, mut conversation, event)) = settlement {
            let mut settled = mutation.clone();
            conversation.last_seq = settled.agent.conversation.last_seq;
            settled.agent.conversation = conversation;
            bump_log_sequence(&mut settled.agent, 1)?;
            settled.agent_logs.insert(0, event);
            match store.try_publish_agent_work(settled, parents).await {
                Ok(AgentWorkPublication::Published) => return Ok(()),
                Ok(AgentWorkPublication::KnownAborted) => return Err(SandboxStoreError::Conflict),
                // The additional terminal event must never consume the final Stop reserve.
                Err(SandboxStoreError::AgentLogCapacityExceeded) => {}
                Err(error) => return Err(error),
            }
        }
        store.commit_agent_work(mutation).await
    }
}
