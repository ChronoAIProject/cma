//! Consume the just-opened channel in the same call, retaining a fresh writer claim.
use super::*;
use crate::sandbox::agent_worker::startup::validated_open_request;
use crate::sandbox::agent_worker::support::validate_lineage;

impl AgentWorker {
    pub(in crate::sandbox::agent_worker) async fn publish_prepared_connection(
        &self,
        opened: &Agent,
        observed: &AgentContext,
        original: &LiveAgentAttachment,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        // Ready publication releases the old lease. Never use it to publish a
        // channel: this claim serializes with checkpoint/pause and other writers.
        let Some(agent) = self
            .dependencies
            .store
            .claim_agent_work(
                &opened.id,
                &self.config.holder,
                rfc3339_at(now),
                rfc3339_at(now + self.config.lease_duration),
            )
            .await?
        else {
            return Ok(());
        };
        let claimed = agent.clone();
        let result = async {
            if agent.work.step != AgentWorkStep::PublishConnection {
                // A concurrent command may have scheduled cleanup before our claim.
                return Box::pin(self.execute(agent, now + started.elapsed())).await;
            }
            let context = self
                .prepared_connection_context(&agent, observed)
                .await
                .ok()
                .flatten();
            // Parent reads may stall beyond the new lease. Do not activate or fall
            // back with expired authority; durable PublishConnection remains due.
            if !agent.work.lease.as_ref().is_some_and(|lease| {
                live_lease(lease, &self.config.holder, now + started.elapsed())
            }) {
                return Err(SandboxStoreError::Conflict);
            }
            let transferred = context.as_ref().and_then(|context| {
                if !same_opened_agent(opened, &agent, original)
                    || validated_open_request(&agent, context).is_err()
                {
                    return None;
                }
                let previous = original.identity();
                let target = LiveAgentTarget {
                    agent_id: agent.id.clone(),
                    conversation_id: previous.conversation_id.clone(),
                    runtime_generation: previous.runtime_generation,
                };
                let staged = self
                    .dependencies
                    .registry
                    .staged_attachment(&target, previous.authority_fence)
                    .ok()??;
                if staged.identity() != previous {
                    return None;
                }
                let fence = claimed_fence(&agent, &self.config.holder).ok()?;
                let transferred = self
                    .dependencies
                    .registry
                    .staged_attachment(&target, fence)
                    .ok()??;
                let current = transferred.identity();
                (current.nonce == previous.nonce
                    && current.conversation_id == previous.conversation_id
                    && current.runtime_generation == previous.runtime_generation)
                    .then_some(transferred)
            });
            let Some(attachment) = transferred else {
                return self
                    .publish_connection(agent, now + started.elapsed())
                    .await;
            };
            if self
                .dependencies
                .registry
                .publish_and_activate(&attachment)
                .is_err()
            {
                return self
                    .publish_connection(agent, now + started.elapsed())
                    .await;
            }
            self.finish_connection_publication(
                agent,
                &attachment,
                context.as_ref().map(AgentContext::publication_parents),
                now + started.elapsed(),
            )
            .await?;
            let identity = attachment.identity();
            let live_connected =
                self.dependencies
                    .registry
                    .published_target_is_attached(&LiveAgentTarget {
                        agent_id: identity.agent_id.clone(),
                        conversation_id: identity.conversation_id,
                        runtime_generation: identity.runtime_generation,
                    });
            tracing::info!(
                agent_id = %identity.agent_id,
                authority_fence = identity.authority_fence,
                elapsed_ms = started.elapsed().as_millis() as u64,
                live_connected,
                "Prepared Agent connection publication completed"
            );
            Ok(())
        }
        .await;
        self.finish_capacity_attempt(&claimed, result, now + started.elapsed())
            .await
    }

    async fn prepared_connection_context(
        &self,
        agent: &Agent,
        observed: &AgentContext,
    ) -> Result<Option<AgentContext>, SandboxStoreError> {
        let (sandbox, workspace, worktree) = tokio::join!(
            self.dependencies.store.get_sandbox(&agent.sandbox_id),
            self.dependencies.store.get_workspace(&agent.workspace_id),
            self.dependencies.store.get_worktree(&agent.worktree_id),
        );
        let (Some(sandbox), Some(workspace), Some(worktree)) = (sandbox?, workspace?, worktree?)
        else {
            return Ok(None);
        };
        let mut expected_tree = observed.worktree.clone();
        expected_tree
            .transition(WorktreeStatus::Ready)
            .map_err(|_| SandboxStoreError::Conflict)?;
        // Full equality includes owner, credential custody, runtime boot and
        // generation, persistent home authority, and lifecycle/checkpoint state.
        if sandbox != observed.sandbox
            || workspace != observed.workspace
            || worktree != expected_tree
            || validate_lineage(agent, &sandbox, &workspace, &worktree).is_err()
        {
            return Ok(None);
        }
        Ok(Some(AgentContext {
            sandbox,
            workspace,
            worktree,
            runtime: observed.runtime.clone(),
            agent_resource_preparation: observed.agent_resource_preparation,
        }))
    }
}

fn same_opened_agent(opened: &Agent, agent: &Agent, original: &LiveAgentAttachment) -> bool {
    let identity = original.identity();
    agent.status == AgentStatus::Ready
        && agent.id == opened.id
        && agent.requested_stop_reason.is_none()
        && agent.deleted_at.is_none()
        && agent.owner == opened.owner
        && agent.sandbox_id == opened.sandbox_id
        && agent.workspace_id == opened.workspace_id
        && agent.worktree_id == opened.worktree_id
        && agent.origin == opened.origin
        && agent.creation_key == opened.creation_key
        && agent.profile == opened.profile
        && agent.capabilities == opened.capabilities
        && agent.conversation.settings.is_none()
        && agent.conversation.conversation_id.as_deref() == Some(identity.conversation_id.as_str())
        && agent.conversation.runtime_generation == Some(identity.runtime_generation)
        && agent.work.warm_conversation_id == opened.work.warm_conversation_id
        && agent.work.resolved_skills == opened.work.resolved_skills
        && agent.work.live_skills.is_empty()
        && agent.work.receipts.get(..opened.work.receipts.len())
            == Some(opened.work.receipts.as_slice())
        && agent.work.lease.as_ref().is_some_and(|lease| {
            lease.fence == opened.work.next_fence && lease.fence > identity.authority_fence
        })
        && agent.work.attempts == 0
}

fn live_lease(lease: &layer2_core::sandbox::AgentWorkLease, holder: &str, now: SystemTime) -> bool {
    lease.holder == holder
        && lease.fence > 0
        && layer2_core::seconds_from_rfc3339(&lease.expires_at).is_some_and(|seconds| {
            SystemTime::UNIX_EPOCH
                .checked_add(std::time::Duration::from_secs(seconds))
                .is_some_and(|expires| now < expires)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_transfer_requires_an_unexpired_exact_writer_lease() {
        let mut lease = layer2_core::sandbox::AgentWorkLease {
            holder: "worker".into(),
            fence: 3,
            expires_at: "2026-09-05T00:01:00Z".into(),
        };
        let expires = SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(
                layer2_core::seconds_from_rfc3339(&lease.expires_at).unwrap(),
            );
        assert!(live_lease(
            &lease,
            "worker",
            expires - std::time::Duration::from_secs(1)
        ));
        assert!(!live_lease(&lease, "worker", expires));
        assert!(!live_lease(
            &lease,
            "worker",
            expires + std::time::Duration::from_secs(1)
        ));
        assert!(!live_lease(
            &lease,
            "other",
            expires - std::time::Duration::from_secs(1)
        ));
        lease.expires_at = "invalid".into();
        assert!(!live_lease(&lease, "worker", expires));
    }
}
