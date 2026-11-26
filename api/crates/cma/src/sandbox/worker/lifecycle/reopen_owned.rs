//! Reopen under the owner epoch and activate only after durable publication.
use super::super::SandboxWorker;
use super::reopen_output::{drain, output_refused};
use crate::sandbox::{live_agents::LiveAgentAttachment, runtime_capabilities, runtime_options};
use layer2_core::{
    agent_ownership::execution_authority,
    now_rfc3339,
    sandbox::{
        store::{AgentReopenGuard, SandboxMutationGuard},
        AgentStatus, AgentWorkReceipt, AgentWorkStep, SandboxStatus, WorktreeStatus,
    },
    AgentId,
};
use layer3_adapter::{AdapterError, AgentOpenRequest};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::Mutex;
use tokio::time::Instant;

mod claim;
use claim::Claim;

pub(in crate::sandbox::worker) type ReopenGates = BTreeMap<AgentId, Weak<Mutex<()>>>;

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
        let gate = {
            let mut gates = self.reopen_gates.lock().unwrap();
            gates.retain(|_, gate| gate.strong_count() != 0);
            gates.get(id).and_then(Weak::upgrade).unwrap_or_else(|| {
                let gate = Arc::new(Mutex::new(()));
                gates.insert(id.clone(), Arc::downgrade(&gate));
                gate
            })
        };
        let _serial = gate.lock().await;
        let agents = self
            .reopen_agents
            .get()
            .ok_or_else(|| refuse_at(id, "agents_unavailable"))?
            .clone();
        let authority = execution_authority()
            .filter(|owner| owner.token.agent_id == *id)
            .ok_or_else(|| refuse_at(id, "owner_authority"))?;
        let sandbox = self
            .dependencies
            .store
            .get_sandbox(&call.sandbox_id)
            .await
            .map_err(|_| refuse_at(id, "sandbox_read"))?
            .ok_or_else(|| refuse_at(id, "sandbox_missing"))?;
        if sandbox.revision != call.revision
            || sandbox.work.lease.as_ref().is_none_or(|lease| {
                lease.holder != call.lease.holder
                    || lease.fence != call.lease.fence
                    || lease.expires_at < call.lease.expires_at
            })
        {
            return Err(refuse_at(id, "caller_lease_changed"));
        }
        let deadline =
            Instant::now() + Duration::from_secs(layer3_adapter::DEFAULT_TOTAL_TIMEOUT_SECONDS);
        let agent = agents
            .get_agent(id)
            .await
            .map_err(|_| refuse_at(id, "agent_read"))?
            .ok_or_else(|| refuse_at(id, "agent_missing"))?;
        if !sandbox.restores_agent(&agent) {
            return Err(refuse_at(id, "agent_not_restored"));
        }
        let generation = sandbox
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.provision_generation)
            .ok_or_else(|| refuse_at(id, "runtime_generation"))?;
        let conversation = agent
            .conversation
            .conversation_id
            .clone()
            .ok_or_else(|| refuse_at(id, "conversation_missing"))?;
        if sandbox.retained_home_recovery().is_none()
            && !super::checkpoint::pinned_checkpoint(&sandbox)
                .map_err(|_| refuse_at(id, "checkpoint_read"))?
                .conversations
                .iter()
                .any(|captured| {
                    captured.agent_id == *id && captured.conversation_id == conversation
                })
        {
            return Err(refuse_at(id, "checkpoint_conversation"));
        }
        let guard = AgentReopenGuard {
            owner_restart: None,
            sandbox: SandboxMutationGuard::capture(&sandbox),
            agent_fence: agent.work.next_fence,
            native_conversation_id: conversation.clone(),
            runtime_generation: generation,
        };
        guard
            .validate(&sandbox, &agent, &now_rfc3339())
            .map_err(|_| refuse_at(id, "guard_invalid"))?;
        {
            let mut pending = self.pending_reopens.lock().unwrap();
            if let Some(previous) = pending.get(id) {
                if previous.guard.matches(&sandbox) && previous.owner == authority.token {
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
            .map_err(|_| refuse_at(id, "worktree_read"))?
            .ok_or_else(|| refuse_at(id, "worktree_missing"))?;
        if worktree.status != WorktreeStatus::Ready
            || agent.validate_prerequisites(&worktree).is_err()
        {
            return Err(refuse_at(id, "worktree_not_ready"));
        }
        let runtime = super::super::support::runtime(&sandbox)
            .ok_or_else(|| refuse_at(id, "runtime_binding"))?;
        let request = AgentOpenRequest::new(
            format!("agent:{id}:open:v1"),
            runtime_options(&agent, &sandbox, &worktree),
            None,
        );
        let (updates, mut receiver) = tokio::sync::mpsc::channel(256);
        let mut claim = Claim::new(self, &guard.sandbox, &sandbox)?;
        let channel = tokio::time::timeout_at(deadline, async {
            claim
                .follow(crate::sandbox::runtime_attachment::adopt_bound_runtime(
                    self.dependencies.adapter.as_ref(),
                    &sandbox,
                    &self.config.deployment_id,
                ))
                .await
                .map_err(reopen_failed(id, "adopt_runtime"))?;
            if !authority.is_current() || Instant::now() >= claim.expires() {
                return Err(refuse_at(id, "reconnect_authority"));
            }
            let reconnect = claim
                .follow(drain(
                    self.dependencies.adapter.reconnect_agent(
                        &runtime,
                        agent.work.next_fence,
                        &request,
                        &conversation,
                        updates,
                    ),
                    &mut receiver,
                    self.dependencies.updates.as_ref(),
                    id,
                    &guard,
                    self.reopen_output_refusal_enabled(&sandbox),
                ))
                .await;
            if reconnect.as_ref().is_err_and(output_refused) {
                let parent = claim.read().await?;
                claim.observe(&parent)?;
                if !authority.is_current() || !self.reopen_output_refusal_enabled(&parent) {
                    return Err(refuse_at(id, "refusal_authority"));
                }
            }
            let channel = reconnect.map_err(reopen_failed(id, "reconnect"))?;
            if channel.conversation_id.as_deref() != Some(&conversation) {
                return Err(reopen_failed(id, "conversation_mismatch")(
                    AdapterError::Rejected(
                        "runtime returned a different retained conversation identity".into(),
                    ),
                ));
            }
            if let Some(settings) = &agent.conversation.settings {
                if !authority.is_current() || Instant::now() >= claim.expires() {
                    return Err(refuse_at(id, "settings_authority"));
                }
                claim
                    .follow(drain(
                        channel.input.send(settings.runtime_input()),
                        &mut receiver,
                        self.dependencies.updates.as_ref(),
                        id,
                        &guard,
                        false,
                    ))
                    .await
                    .map_err(reopen_failed(id, "settings"))?;
            }
            Ok(channel)
        })
        .await
        .map_err(|_| refuse_at(id, "reopen_deadline"))??;
        if !authority.is_current() || Instant::now() >= claim.expires() {
            return Err(refuse_at(id, "publication_authority"));
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
            .map_err(|_| refuse_at(id, "registry_attach"))?;
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
            let observed_expiry = claim.expires();
            tokio::spawn(async move {
                let watching = async {
                    let mut delay = Duration::from_millis(100);
                    let mut claim =
                        Claim::from_observation(&cleanup.worker, &guard.sandbox, observed_expiry);
                    loop {
                        let parent = claim.read_staged_parent().await?;
                        if guard.sandbox.matches(&parent) {
                            claim.observe(&parent)?;
                        }
                        let current = claim.read_staged(|| agents.get_agent(&id)).await?;
                        if let Some(current) = current {
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
                                // Retirement may follow the first parent
                                // read while this Agent still reports Ready.
                                let published = claim.read_staged_parent().await?;
                                if !matches!(
                                    published.status,
                                    SandboxStatus::Reviving
                                        | SandboxStatus::Rotating
                                        | SandboxStatus::Active
                                ) || published
                                    .runtime
                                    .as_ref()
                                    .and_then(|binding| binding.provision_generation)
                                    != Some(generation)
                                    || !authority.is_current()
                                {
                                    return Err(refused());
                                }
                                cleanup
                                    .worker
                                    .dependencies
                                    .registry
                                    .publish_and_activate(&cleanup.attachment)
                                    .map_err(|_| refused())?;
                                return Ok(());
                            }
                            if current.status != AgentStatus::Paused
                                || current.work.next_fence != guard.agent_fence
                            {
                                return Err(refused());
                            }
                        } else {
                            return Err(refused());
                        }
                        claim.observe(&parent)?;
                        tokio::time::sleep_until(claim.next_read_at(delay).min(claim.expires()))
                            .await;
                        delay = (delay * 2).min(Duration::from_secs(2));
                    }
                };
                if !matches!(authority.run(watching).await, Ok(Ok(()))) {
                    tracing::debug!(agent_id = %id, "owner reopen stage ended before publication");
                }
                // Drop rolls back only this exact staged nonce, never a published
                // entry or a newer owner attachment, including cancellation.
                drop(cleanup);
            });
        }
        Ok(receipt)
    }
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

fn refused() -> AdapterError {
    AdapterError::Unavailable("lifecycle reopen authority changed".into())
}

fn refuse_at(id: &AgentId, reason: &'static str) -> AdapterError {
    tracing::warn!(agent_id = %id, reason, "Owner lifecycle reopen refused");
    refused()
}

fn reopen_failed<'a>(
    id: &'a AgentId,
    stage: &'static str,
) -> impl FnOnce(AdapterError) -> AdapterError + 'a {
    move |error| {
        tracing::warn!(agent_id = %id, stage, error = %error, "Owner lifecycle reopen failed");
        error
    }
}
