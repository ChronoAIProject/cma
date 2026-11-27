use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    AgentReopenGuard, SandboxLifecycleAgentMutation, SandboxLifecycleAgentScope,
    SandboxMutationGuard, SandboxStoreError, WorktreeStore,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkReceipt, AgentWorkStep, FirstMessageDelivery,
    Sandbox, SandboxEventKind, SandboxWorkStep, Worktree, WorktreeStatus,
};
use layer3_adapter::AgentOpenRequest;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::super::support::{runtime, WorkerFailure};
use super::super::SandboxWorker;
use super::checkpoint::{pinned_checkpoint, require_quiet_agent};
use super::reopen_refusal::within_deadline;
use super::support::{lifecycle_lease, model_error};
use crate::sandbox::live_agents::{LiveAgentAttachment, LiveAgentRegistryError};
use crate::sandbox::runtime_mapping::{runtime_capabilities, runtime_options};

const UPDATE_BUFFER: usize = 256;
const REOPEN_LIMIT: Duration = Duration::from_secs(layer3_adapter::DEFAULT_TOTAL_TIMEOUT_SECONDS);

impl SandboxWorker {
    pub(super) async fn reopen_lifecycle_conversations(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if let Some(failure) = self.staging_runtime_update_refusal(&sandbox) {
            return self.handle_lifecycle_failure(sandbox, failure, now).await;
        }
        if let Err(failure) = self.reconcile_retired_runtime_paths(&sandbox, now).await {
            return self.handle_lifecycle_failure(sandbox, failure, now).await;
        }
        let Some(runtime) = runtime(&sandbox) else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("replacement runtime binding is absent"),
                    now,
                )
                .await;
        };
        let checkpoint = match pinned_checkpoint(&sandbox) {
            Ok(checkpoint) if sandbox.retained_home_recovery().is_none() => Some(checkpoint),
            _ if sandbox.retained_home_recovery().is_some() => None,
            Ok(_) => unreachable!("archive recovery was matched"),
            Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
        };
        let agents = self.lifecycle_agents(&sandbox).await?;
        let live = match validate_reopen_set(&sandbox, &agents, checkpoint) {
            Ok(live) => live,
            Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
        };
        let worktrees = self.lifecycle_worktrees(&live).await?;
        let generation = match sandbox
            .runtime
            .as_ref()
            .and_then(|binding| binding.provision_generation)
        {
            Some(generation) => generation,
            None => {
                return self
                    .handle_lifecycle_failure(
                        sandbox,
                        WorkerFailure::terminal_security(
                            "replacement runtime generation is absent",
                        ),
                        now,
                    )
                    .await
            }
        };

        let mut attachments = Vec::new();
        let mut receipts = BTreeMap::new();
        // Earlier restore phases have their own budgets, and an empty set has
        // no conversation work to time out.
        let deadline = if live.is_empty() {
            Instant::now()
        } else {
            self.conversation_reopen_deadline(&sandbox).await?
        };
        for agent in &live {
            let refusal = match self.reopen_refusal_fence(&sandbox, agent).await {
                Ok(refusal) => refusal,
                Err(error) => {
                    rollback_attachments(self, &attachments);
                    return Err(error);
                }
            };
            let prove_refusal = refusal.is_some();
            if let Some(relay) = self.reopen_relay.get() {
                let relay = relay.upgrade().ok_or(SandboxStoreError::Conflict)?;
                match within_deadline(
                    deadline,
                    relay.reopen_agent(&sandbox, &agent.id, prove_refusal),
                    refusal,
                    true,
                )
                .await
                {
                    Ok(mut receipt) => {
                        // Lifecycle validation uses the publisher's logical
                        // timestamp, which can differ from the remote clock.
                        receipt.completed_at = rfc3339_at(now);
                        receipts.insert(agent.id.clone(), receipt);
                    }
                    Err(failure) => {
                        tracing::warn!(sandbox_id = %sandbox.id, agent_id = %agent.id,
                            stage = "relayed_reopen", error = %failure.detail(),
                            "Lifecycle conversation reopen failed");
                        return self
                            .rollback_reopen(sandbox, attachments, failure, now)
                            .await;
                    }
                }
                continue;
            }
            let worktree = worktrees
                .get(&agent.id)
                .expect("every validated live agent has a worktree");
            let conversation_id = agent
                .conversation
                .conversation_id
                .clone()
                .expect("validated live agents have conversation identities");
            let request = AgentOpenRequest::new(
                format!("agent:{}:open:v1", agent.id),
                runtime_options(agent, &sandbox, worktree),
                None,
            );
            let authority_fence = agent.work.next_fence;
            let guard = AgentReopenGuard {
                owner_restart: None,
                sandbox: SandboxMutationGuard::capture(&sandbox),
                agent_fence: authority_fence,
                native_conversation_id: conversation_id.clone(),
                runtime_generation: generation,
            };
            let (updates, mut receiver) = mpsc::channel(UPDATE_BUFFER);
            let opening = self.dependencies.adapter.reconnect_agent(
                &runtime,
                authority_fence,
                &request,
                &conversation_id,
                updates,
            );
            let channel = match within_deadline(
                deadline,
                super::reopen_output::drain_while(
                    opening,
                    &mut receiver,
                    self.dependencies.updates.as_ref(),
                    &agent.id,
                    &guard,
                    prove_refusal,
                ),
                refusal,
                false,
            )
            .await
            {
                Ok(channel) if channel.conversation_id.as_deref() == Some(&conversation_id) => {
                    channel
                }
                Ok(_) => {
                    return self
                        .rollback_reopen(
                            sandbox,
                            attachments,
                            WorkerFailure::terminal_security(
                                "runtime reopened a different conversation identity",
                            ),
                            now,
                        )
                        .await
                }
                Err(failure) => {
                    tracing::warn!(sandbox_id = %sandbox.id, agent_id = %agent.id,
                        stage = "reconnect", error = %failure.detail(),
                        "Lifecycle conversation reopen failed");
                    return self
                        .rollback_reopen(sandbox, attachments, failure, now)
                        .await;
                }
            };
            if let Some(settings) = &agent.conversation.settings {
                if let Err(failure) = within_deadline(
                    deadline,
                    super::reopen_output::drain_while(
                        channel.input.send(settings.runtime_input()),
                        &mut receiver,
                        self.dependencies.updates.as_ref(),
                        &agent.id,
                        &guard,
                        false,
                    ),
                    None,
                    false,
                )
                .await
                {
                    return self
                        .rollback_reopen(sandbox, attachments, failure, now)
                        .await;
                }
            }
            let attachment = match self.dependencies.registry.attach(
                agent.id.clone(),
                conversation_id,
                generation,
                authority_fence,
                runtime_capabilities(self.dependencies.adapter.capabilities()),
                channel.input,
                receiver,
                self.dependencies.updates.clone(),
            ) {
                Ok(attachment) => attachment,
                Err(error) => {
                    return self
                        .rollback_reopen(sandbox, attachments, registry_failure(error), now)
                        .await
                }
            };
            attachments.push(attachment);
            receipts.insert(
                agent.id.clone(),
                AgentWorkReceipt {
                    step: AgentWorkStep::OpenConversation,
                    operation_id: request.operation_id,
                    request_sha256: request.request_sha256,
                    completed_at: rfc3339_at(now),
                },
            );
        }

        // Startup metadata may have appended durable logs. Publish from those
        // current rows so the final CAS cannot erase already acknowledged output.
        let refreshed = self.lifecycle_agents(&sandbox).await?;
        let refreshed_live = match validate_reopen_set(&sandbox, &refreshed, checkpoint) {
            Ok(live) => live,
            Err(failure) => {
                return self
                    .rollback_reopen(sandbox, attachments, failure, now)
                    .await
            }
        };
        if refreshed_live.len() != live.len()
            || refreshed_live.iter().any(|agent| {
                !live.iter().any(|original| {
                    original.id == agent.id
                        && original.conversation.conversation_id
                            == agent.conversation.conversation_id
                        && original.work.next_fence == agent.work.next_fence
                })
            })
        {
            rollback_attachments(self, &attachments);
            return Err(SandboxStoreError::Conflict);
        }
        let live = refreshed_live;
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_lifecycle_conversation_reopen(&lease.holder, lease.fence, stamp.clone())
            .map_err(model_error)?;
        let mut mutations = Vec::new();
        let mut events = Vec::new();
        for current in &live {
            let conversation_id = current
                .conversation
                .conversation_id
                .clone()
                .expect("validated live agents have conversation identities");
            let mut agent = (*current).clone();
            agent.conversation.runtime_generation = Some(generation);
            // Publish the exact successful reopen request with the new generation.
            // Another replica must be able to reattach before queue settlement.
            agent
                .work
                .receipts
                .retain(|receipt| receipt.step != AgentWorkStep::OpenConversation);
            agent.work.receipts.push(
                receipts
                    .remove(&agent.id)
                    .expect("every reopened agent has a receipt"),
            );
            agent
                .transition(AgentStatus::Ready, stamp.clone(), None)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            if agent
                .conversation
                .first_message
                .as_ref()
                .is_some_and(|message| message.state == FirstMessageDelivery::Queued)
            {
                agent
                    .work
                    .schedule(AgentWorkStep::DispatchFirstMessage, stamp.clone());
            } else if agent.work.live_skills.last().is_some_and(|skill| {
                skill.status == layer2_core::sandbox::AgentSkillInstallStatus::Pending
            }) {
                agent
                    .work
                    .schedule(AgentWorkStep::InstallSkill, stamp.clone());
            }
            let logs = vec![
                AgentLogKind::ConversationOpened {
                    conversation_id,
                    runtime_generation: generation,
                },
                AgentLogKind::StatusChanged {
                    from: current.status,
                    to: AgentStatus::Ready,
                },
            ];
            agent.conversation.last_seq = agent
                .conversation
                .last_seq
                .checked_add(2)
                .ok_or(SandboxStoreError::SequenceConflict)?;
            events.push(SandboxEventKind::AgentStatusChanged {
                agent_id: current.id.clone(),
                from: current.status,
                to: AgentStatus::Ready,
            });
            mutations.push(SandboxLifecycleAgentMutation {
                expected_revision: current.revision,
                agent,
                logs,
            });
        }
        events.push(SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        });
        let committed = self
            .commit_lifecycle_phase(
                &sandbox,
                next.clone(),
                mutations,
                SandboxLifecycleAgentScope::RestoreParticipants,
                stamp,
                events,
            )
            .await;
        let durable = match committed {
            Ok(()) => true,
            Err(error) => match self.lifecycle_state_matches(&next).await {
                Ok(true) => true,
                Ok(false) => {
                    rollback_attachments(self, &attachments);
                    return Err(error);
                }
                Err(reconcile_error) => {
                    rollback_attachments(self, &attachments);
                    return Err(reconcile_error);
                }
            },
        };
        debug_assert!(durable);
        for attachment in &attachments {
            self.dependencies
                .registry
                .publish_and_activate(attachment)
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        }
        Ok(())
    }

    async fn lifecycle_worktrees(
        &self,
        agents: &[&Agent],
    ) -> Result<BTreeMap<layer2_core::AgentId, Worktree>, SandboxStoreError> {
        let mut worktrees = BTreeMap::new();
        for agent in agents {
            let worktree =
                WorktreeStore::get_worktree(self.dependencies.store.as_ref(), &agent.worktree_id)
                    .await?
                    .ok_or(SandboxStoreError::NotFound)?;
            if worktree.status != WorktreeStatus::Ready
                || agent.validate_prerequisites(&worktree).is_err()
            {
                return Err(SandboxStoreError::Corrupt(
                    "restored agent worktree lineage is invalid".into(),
                ));
            }
            worktrees.insert(agent.id.clone(), worktree);
        }
        Ok(worktrees)
    }

    async fn conversation_reopen_deadline(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Instant, SandboxStoreError> {
        let remaining = if sandbox.work.attempts == 0 {
            REOPEN_LIMIT
        } else {
            let operation = sandbox
                .lifecycle
                .operation
                .as_ref()
                .ok_or(SandboxStoreError::Conflict)?;
            let first_attempt = self
                .dependencies
                .store
                .lifecycle_retry_started_at(&sandbox.id, &operation.authority().requested_at)
                .await?;
            if let Some(first_attempt) = first_attempt {
                let seconds =
                    layer2_core::seconds_from_rfc3339(&first_attempt).ok_or_else(|| {
                        SandboxStoreError::Corrupt("invalid lifecycle retry timestamp".into())
                    })?;
                let started = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
                REOPEN_LIMIT.saturating_sub(
                    SystemTime::now()
                        .duration_since(started)
                        .unwrap_or_default(),
                )
            } else {
                REOPEN_LIMIT
            }
        };
        Ok(Instant::now() + remaining)
    }

    async fn rollback_reopen(
        &self,
        sandbox: Sandbox,
        attachments: Vec<LiveAgentAttachment>,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        rollback_attachments(self, &attachments);
        self.handle_lifecycle_failure(sandbox, failure, now).await
    }
}

fn validate_reopen_set<'a>(
    sandbox: &Sandbox,
    agents: &'a [Agent],
    checkpoint: Option<&layer2_core::sandbox::SandboxCheckpointRef>,
) -> Result<Vec<&'a Agent>, WorkerFailure> {
    let by_id = checkpoint
        .into_iter()
        .flat_map(|checkpoint| &checkpoint.conversations)
        .map(|conversation| {
            (
                &conversation.agent_id,
                conversation.conversation_id.as_str(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let live = agents
        .iter()
        .filter(|agent| sandbox.restores_agent(agent))
        .collect::<Vec<_>>();
    let matches = live.iter().all(|agent| {
        agent.status == AgentStatus::Paused
            && require_quiet_agent(agent).is_ok()
            && agent
                .conversation
                .conversation_id
                .as_deref()
                .is_some_and(|current| {
                    sandbox.retained_home_recovery().is_some()
                        || by_id.get(&agent.id).copied() == Some(current)
                })
    });
    if matches {
        Ok(live)
    } else {
        Err(WorkerFailure::terminal_security(
            "restored live agents do not match the pinned checkpoint conversations",
        ))
    }
}

fn rollback_attachments(worker: &SandboxWorker, attachments: &[LiveAgentAttachment]) {
    for attachment in attachments {
        worker.dependencies.registry.rollback(attachment);
    }
}

fn registry_failure(error: LiveAgentRegistryError) -> WorkerFailure {
    match error {
        LiveAgentRegistryError::ConversationMismatch => WorkerFailure::terminal_security(
            "replacement runtime conversation identity conflicts with the registry",
        ),
        LiveAgentRegistryError::StaleGeneration | LiveAgentRegistryError::NotAttached => {
            WorkerFailure::retryable("replacement runtime attachment authority is stale")
        }
    }
}
