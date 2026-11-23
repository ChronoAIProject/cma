//! Reopen under the owner epoch and activate only after durable publication.
use super::super::SandboxWorker;
use super::reopen_output::{drain_while, drain_while_held, RELAYED_REOPEN_LIMIT};
use crate::sandbox::{live_agents::LiveAgentAttachment, runtime_capabilities, runtime_options};
use layer2_core::{
    agent_ownership::execution_authority,
    now_rfc3339,
    sandbox::{
        store::{AgentReopenGuard, SandboxMutationGuard},
        AgentStatus, AgentWorkReceipt, AgentWorkStep, WorktreeStatus,
    },
    AgentId,
};
use layer3_adapter::{AdapterError, AgentOpenRequest};
use std::{sync::Arc, time::Duration};

pub(in crate::sandbox::worker) struct PendingReopen {
    owner: layer2_core::agent_ownership::OwnershipToken,
    guard: SandboxMutationGuard,
    receipt: AgentWorkReceipt,
    attachment: LiveAgentAttachment,
}

impl SandboxWorker {
    pub(crate) async fn reopen_at_owner(
        self: &Arc<Self>,
        id: &AgentId,
        call: &crate::agent_manager::relay::ReopenCall,
    ) -> layer3_adapter::Result<AgentWorkReceipt> {
        let _serial = self.reopen_gate.lock().await;
        let agents = self
            .reopen_agents
            .get()
            .ok_or_else(|| refused(id, "agents_unavailable"))?
            .clone();
        let authority = execution_authority()
            .filter(|owner| owner.token.agent_id == *id)
            .ok_or_else(|| refused(id, "owner_authority"))?;
        let sandbox = self
            .dependencies
            .store
            .get_sandbox(&call.sandbox_id)
            .await
            .map_err(|_| refused(id, "sandbox_read"))?
            .ok_or_else(|| refused(id, "sandbox_missing"))?;
        if sandbox.revision != call.revision
            || sandbox.work.lease.as_ref().is_none_or(|lease| {
                lease.holder != call.lease.holder
                    || lease.fence != call.lease.fence
                    || lease.expires_at < call.lease.expires_at
            })
        {
            return Err(refused(id, "caller_lease_changed"));
        }
        let agent = agents
            .get_agent(id)
            .await
            .map_err(|_| refused(id, "agent_read"))?
            .ok_or_else(|| refused(id, "agent_missing"))?;
        if !sandbox.restores_agent(&agent) {
            return Err(refused(id, "agent_not_restored"));
        }
        let generation = sandbox
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.provision_generation)
            .ok_or_else(|| refused(id, "runtime_generation"))?;
        let conversation = agent
            .conversation
            .conversation_id
            .clone()
            .ok_or_else(|| refused(id, "conversation_missing"))?;
        if sandbox.retained_home_recovery().is_none()
            && !super::checkpoint::pinned_checkpoint(&sandbox)
                .map_err(|_| refused(id, "checkpoint_read"))?
                .conversations
                .iter()
                .any(|captured| {
                    captured.agent_id == *id && captured.conversation_id == conversation
                })
        {
            return Err(refused(id, "checkpoint_conversation"));
        }
        let guard = AgentReopenGuard {
            sandbox: SandboxMutationGuard::capture(&sandbox),
            agent_fence: agent.work.next_fence,
            native_conversation_id: conversation.clone(),
            runtime_generation: generation,
        };
        guard
            .validate(&sandbox, &agent, &now_rfc3339())
            .map_err(|_| refused(id, "guard_invalid"))?;
        {
            let mut pending = self.pending_reopens.lock().unwrap();
            if let Some(previous) = pending.get(id) {
                if previous.guard == guard.sandbox && previous.owner == authority.token {
                    return Ok(previous.receipt.clone());
                }
            }
            if let Some(previous) = pending.remove(id) {
                self.dependencies.registry.rollback(&previous.attachment);
            }
        }
        let worktree = self
            .dependencies
            .store
            .get_worktree(&agent.worktree_id)
            .await
            .map_err(|_| refused(id, "worktree_read"))?
            .ok_or_else(|| refused(id, "worktree_missing"))?;
        if worktree.status != WorktreeStatus::Ready
            || agent.validate_prerequisites(&worktree).is_err()
        {
            return Err(refused(id, "worktree_not_ready"));
        }
        crate::sandbox::runtime_attachment::adopt_bound_runtime(
            self.dependencies.adapter.as_ref(),
            &sandbox,
            &self.config.deployment_id,
        )
        .await
        .map_err(reopen_failed(id, "adopt_runtime"))?;
        let runtime = super::super::support::runtime(&sandbox)
            .ok_or_else(|| refused(id, "runtime_binding"))?;
        let request = AgentOpenRequest::new(
            format!("agent:{id}:open:v1"),
            runtime_options(&agent, &sandbox, &worktree),
            None,
        );
        let (updates, mut receiver) = tokio::sync::mpsc::channel(256);
        let channel = self
            .drain_reopen(
                self.dependencies.adapter.reconnect_agent(
                    &runtime,
                    agent.work.next_fence,
                    &request,
                    &conversation,
                    updates,
                ),
                &mut receiver,
                id,
                &guard,
            )
            .await
            .map_err(reopen_failed(id, "reconnect"))?;
        if channel.conversation_id.as_deref() != Some(&conversation) {
            return Err(refused(id, "conversation_mismatch"));
        }
        if let Some(settings) = &agent.conversation.settings {
            self.drain_reopen(
                channel.input.send(settings.runtime_input()),
                &mut receiver,
                id,
                &guard,
            )
            .await
            .map_err(reopen_failed(id, "settings"))?;
        }
        let attachment = self
            .dependencies
            .registry
            .attach(
                id.clone(),
                conversation,
                generation,
                agent.work.next_fence,
                runtime_capabilities(self.dependencies.adapter.capabilities()),
                channel.input,
                receiver,
                self.dependencies.updates.clone(),
            )
            .map_err(|_| refused(id, "registry_attach"))?;
        let receipt = AgentWorkReceipt {
            step: AgentWorkStep::OpenConversation,
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
            completed_at: now_rfc3339(),
        };
        // Only the creator watches a staged entry. Retried relay calls may reuse
        // it without accumulating tasks or rolling back another call's stage.
        if attachment.inserted() {
            self.pending_reopens.lock().unwrap().insert(
                id.clone(),
                PendingReopen {
                    owner: authority.token.clone(),
                    guard: guard.sandbox.clone(),
                    receipt: receipt.clone(),
                    attachment: attachment.clone(),
                },
            );
            let cleanup = StagedReopen {
                worker: self.clone(),
                id: id.clone(),
                attachment,
            };
            let receipt = receipt.clone();
            let id = id.clone();
            tokio::spawn(async move {
                let watching = async {
                    let mut delay = Duration::from_millis(100);
                    loop {
                        let parent = cleanup
                            .worker
                            .dependencies
                            .store
                            .get_sandbox(&guard.sandbox.sandbox_id)
                            .await;
                        let current = agents.get_agent(&id).await;
                        if let Ok(Some(current)) = current {
                            if current.status == AgentStatus::Ready
                                && current.conversation.runtime_generation == Some(generation)
                                && current.conversation.conversation_id.as_deref()
                                    == Some(&guard.native_conversation_id)
                                && current.work.next_fence == guard.agent_fence
                                && current.work.receipts.iter().any(|stored| {
                                    stored.step == receipt.step
                                        && stored.operation_id == receipt.operation_id
                                        && stored.request_sha256 == receipt.request_sha256
                                })
                            {
                                let _ = cleanup
                                    .worker
                                    .dependencies
                                    .registry
                                    .publish_and_activate(&cleanup.attachment);
                                return;
                            }
                            if current.status != AgentStatus::Paused
                                || current.work.next_fence != guard.agent_fence
                            {
                                return;
                            }
                        }
                        match parent {
                            // The caller renews its lease in place; end with
                            // that durable lease, not with its call snapshot.
                            Ok(Some(parent))
                                if guard.sandbox.matches(&parent) && lease_live(&parent) => {}
                            Ok(_) => return,
                            Err(_) => (),
                        }
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(Duration::from_secs(5));
                    }
                };
                let _ = authority
                    .run(tokio::time::timeout(RELAYED_REOPEN_LIMIT, watching))
                    .await;
                // Drop rolls back only this exact staged nonce, never a published
                // entry or a newer owner attachment, including cancellation.
                drop(cleanup);
            });
        }
        Ok(receipt)
    }
}

impl SandboxWorker {
    /// A same-process owner keeps the caller's renewed `WorkAuthority`. A
    /// relayed owner has none, so it is bounded by the caller's durable lease
    /// (#3244) instead of the lease snapshot carried by the call.
    async fn drain_reopen<T>(
        &self,
        operation: impl std::future::Future<Output = layer3_adapter::Result<T>>,
        receiver: &mut tokio::sync::mpsc::Receiver<layer3_adapter::ConversationUpdate>,
        id: &AgentId,
        guard: &AgentReopenGuard,
    ) -> layer3_adapter::Result<T> {
        let updates = self.dependencies.updates.as_ref();
        if layer2_core::sandbox::store::WorkAuthority::active() {
            return drain_while(operation, receiver, updates, id, guard).await;
        }
        drain_while_held(operation, receiver, updates, id, guard, || {
            self.caller_lease_held(&guard.sandbox)
        })
        .await
    }

    /// Whether the caller's exact lifecycle lease is still held durably.
    async fn caller_lease_held(&self, guard: &SandboxMutationGuard) -> bool {
        match self.dependencies.store.get_sandbox(&guard.sandbox_id).await {
            Ok(Some(current)) => guard.matches(&current) && lease_live(&current),
            Ok(None) => false,
            // A failed read proves no loss. Durable writes stay fenced at the
            // write, and RELAYED_REOPEN_LIMIT still bounds the work.
            Err(_) => true,
        }
    }
}

/// The sandbox's durable work lease has not expired.
fn lease_live(sandbox: &layer2_core::sandbox::Sandbox) -> bool {
    let now = layer2_core::seconds_from_rfc3339(&now_rfc3339());
    sandbox
        .work
        .lease
        .as_ref()
        .and_then(|lease| layer2_core::seconds_from_rfc3339(&lease.expires_at))
        .zip(now)
        .is_some_and(|(expires, now)| expires > now)
}

struct StagedReopen {
    worker: Arc<SandboxWorker>,
    id: AgentId,
    attachment: LiveAgentAttachment,
}
impl Drop for StagedReopen {
    fn drop(&mut self) {
        let mut pending = self.worker.pending_reopens.lock().unwrap();
        if pending
            .get(&self.id)
            .is_some_and(|entry| entry.attachment == self.attachment)
        {
            pending.remove(&self.id);
        }
        self.worker.dependencies.registry.rollback(&self.attachment);
    }
}

/// Every refusal keeps the caller's retryable error, and logs which closed
/// precondition refused so a reopen that never completes is diagnosable
/// (#3230). The reason is a fixed label; no record content is logged.
fn refused(id: &AgentId, reason: &'static str) -> AdapterError {
    tracing::warn!(agent_id = %id, reason, "Owner lifecycle reopen refused");
    AdapterError::Unavailable("lifecycle reopen authority changed".into())
}

/// Log the class of an adapter failure before it propagates unchanged.
fn reopen_failed<'a>(
    id: &'a AgentId,
    stage: &'static str,
) -> impl FnOnce(AdapterError) -> AdapterError + 'a {
    move |error| {
        tracing::warn!(agent_id = %id, stage, error = %error, "Owner lifecycle reopen failed");
        error
    }
}
