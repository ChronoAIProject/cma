use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    retired_sandbox_cleanup_is_absent, retiring_sandbox_process_is_absent, AgentWorkMutation,
    SandboxStoreError, VersionedWorktree,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkStep, SandboxEventKind, WorktreeStatus,
};
use layer3_adapter::{AgentCloseRequest, AgentHomeRemoveRequest, AgentSlot, WorktreeRef};

use super::startup_support::has_startup_evidence;
use super::support::{
    bump_log_sequence, claimed_fence, request_evidence, validate_lineage, WorkerFailure,
};
use super::AgentWorker;

#[path = "cleanup_retired.rs"]
mod retired;

impl AgentWorker {
    pub(super) async fn fence_interaction(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if !agent.status.is_live() || agent.requested_stop_reason.is_none() {
            return self.invalid_cleanup(agent, now).await;
        }
        let fence = claimed_fence(&agent, &self.config.holder)?;
        self.dependencies
            .registry
            .fence(&agent.id, agent.conversation.runtime_generation, fence);
        self.progress(
            agent,
            AgentWorkStep::CloseProcess,
            now,
            None,
            Vec::new(),
            None,
            None,
        )
        .await
    }

    pub(super) async fn close_process(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if !agent.status.is_live() || agent.requested_stop_reason.is_none() {
            return self.invalid_cleanup(agent, now).await;
        }
        if agent.has_no_runtime_effects() {
            self.unstarted_lineage(&agent).await?;
            return self
                .progress(
                    agent,
                    AgentWorkStep::PublishStopped,
                    now,
                    None,
                    Vec::new(),
                    None,
                    None,
                )
                .await;
        }
        let sandbox = self
            .dependencies
            .store
            .get_sandbox(&agent.sandbox_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if retiring_sandbox_process_is_absent(&agent, &sandbox) {
            match self.dependencies.adapter.runtime_inventory().await {
                Ok(inventory)
                    if inventory
                        .iter()
                        .all(|runtime| runtime.sandbox_id != sandbox.id.as_str()) => {}
                Ok(_) => {
                    return self
                        .handle_failure(
                            agent,
                            WorkerFailure::retryable(
                                "agent_retiring_runtime_present",
                                "the provider still observes a runtime for the retiring sandbox",
                            ),
                            now,
                        )
                        .await;
                }
                Err(error) => {
                    return self
                        .handle_failure(agent, cleanup_failure(error), now)
                        .await
                }
            }
            return self
                .progress(
                    agent,
                    AgentWorkStep::PublishStopped,
                    now,
                    None,
                    Vec::new(),
                    None,
                    None,
                )
                .await;
        }
        let context = match self.cleanup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
        if !has_startup_evidence(&agent, AgentWorkStep::OpenConversation) {
            return self
                .progress(
                    agent,
                    AgentWorkStep::PublishStopped,
                    now,
                    None,
                    Vec::new(),
                    None,
                    None,
                )
                .await;
        }
        let request = close_request(&agent);
        let authority_fence = claimed_fence(&agent, &self.config.holder)?;
        match self
            .dependencies
            .adapter
            .close_agent(&context.runtime, authority_fence, &request)
            .await
        {
            Ok(()) => {
                self.progress(
                    agent,
                    AgentWorkStep::PublishStopped,
                    now,
                    Some(request_evidence(
                        AgentWorkStep::CloseProcess,
                        &request.operation_id,
                        &request.request_sha256,
                    )),
                    Vec::new(),
                    None,
                    None,
                )
                .await
            }
            Err(error) => {
                self.handle_failure(agent, cleanup_failure(error), now)
                    .await
            }
        }
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
        let stamp = rfc3339_at(now);
        let previous = agent.status;
        let mut worktree = self
            .dependencies
            .store
            .get_worktree(&agent.worktree_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
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
                    from: previous,
                    to: AgentStatus::Stopped,
                }),
                agent_logs: vec![AgentLogKind::Stopped {
                    reason: reason.as_str().into(),
                }],
            })
            .await
    }

    pub(super) async fn ensure_process_closed(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if self.complete_retired_cleanup(&agent, now).await? {
            return Ok(());
        }
        if !matches!(agent.status, AgentStatus::Stopped | AgentStatus::Failed) {
            return self.invalid_cleanup(agent, now).await;
        }
        let fence = claimed_fence(&agent, &self.config.holder)?;
        self.dependencies
            .registry
            .fence(&agent.id, agent.conversation.runtime_generation, fence);
        let context = match self.cleanup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
        if !matches!(
            context.worktree.status,
            WorktreeStatus::Ready | WorktreeStatus::Failed
        ) {
            return self.invalid_cleanup(agent, now).await;
        }
        let expected_worktree_revision = context.worktree.revision;
        let mut worktree = context.worktree.clone();
        worktree
            .transition(WorktreeStatus::Removing)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let versioned = Some(VersionedWorktree {
            expected_revision: expected_worktree_revision,
            worktree,
        });

        if has_receipt(&agent, AgentWorkStep::CloseProcess)
            || !has_startup_evidence(&agent, AgentWorkStep::OpenConversation)
        {
            return self
                .progress(
                    agent,
                    AgentWorkStep::RemoveHome,
                    now,
                    None,
                    Vec::new(),
                    versioned,
                    None,
                )
                .await;
        }

        let request = close_request(&agent);
        let authority_fence = claimed_fence(&agent, &self.config.holder)?;
        match self
            .dependencies
            .adapter
            .close_agent(&context.runtime, authority_fence, &request)
            .await
        {
            Ok(()) => {
                self.progress(
                    agent,
                    AgentWorkStep::RemoveHome,
                    now,
                    Some(request_evidence(
                        AgentWorkStep::EnsureProcessClosed,
                        &request.operation_id,
                        &request.request_sha256,
                    )),
                    Vec::new(),
                    versioned,
                    None,
                )
                .await
            }
            Err(error) => {
                self.handle_failure(agent, cleanup_failure(error), now)
                    .await
            }
        }
    }

    pub(super) async fn remove_home(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if self.complete_retired_cleanup(&agent, now).await? {
            return Ok(());
        }
        let context = match self.cleanup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
        if context.worktree.status != WorktreeStatus::Removing {
            return self.invalid_cleanup(agent, now).await;
        }
        if !has_startup_evidence(&agent, AgentWorkStep::MaterializeHome) {
            return self
                .progress(
                    agent,
                    AgentWorkStep::RemoveWorktree,
                    now,
                    None,
                    Vec::new(),
                    None,
                    None,
                )
                .await;
        }
        let request = AgentHomeRemoveRequest::new(
            format!("agent:{}:home:remove:v1", agent.id),
            AgentSlot::new(agent.id.to_string()),
        );
        let authority_fence = claimed_fence(&agent, &self.config.holder)?;
        match self
            .dependencies
            .adapter
            .remove_agent_home(&context.runtime, authority_fence, &request)
            .await
        {
            Ok(()) => {
                self.progress(
                    agent,
                    AgentWorkStep::RemoveWorktree,
                    now,
                    Some(request_evidence(
                        AgentWorkStep::RemoveHome,
                        &request.operation_id,
                        &request.request_sha256,
                    )),
                    Vec::new(),
                    None,
                    None,
                )
                .await
            }
            Err(error) => {
                self.handle_failure(agent, cleanup_failure(error), now)
                    .await
            }
        }
    }

    pub(super) async fn remove_worktree(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if self.complete_retired_cleanup(&agent, now).await? {
            return Ok(());
        }
        let context = match self.cleanup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
        if context.worktree.status != WorktreeStatus::Removing {
            return self.invalid_cleanup(agent, now).await;
        }
        let request = WorktreeRef::new(
            format!(
                "worktree:{}:{}:remove:v1",
                context.workspace.name, context.worktree.name
            ),
            context.workspace.name.clone(),
            context.worktree.name.clone(),
        );
        if has_startup_evidence(&agent, AgentWorkStep::AddWorktree) {
            let authority_fence = claimed_fence(&agent, &self.config.holder)?;
            if let Err(error) = self
                .dependencies
                .adapter
                .remove_worktree(&context.runtime, authority_fence, &request)
                .await
            {
                return self
                    .handle_failure(agent, cleanup_failure(error), now)
                    .await;
            }
        }
        let mut worktree = context.worktree;
        let expected_worktree_revision = worktree.revision;
        worktree
            .transition(WorktreeStatus::Removed)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let evidence = has_startup_evidence(&agent, AgentWorkStep::AddWorktree).then(|| {
            request_evidence(
                AgentWorkStep::RemoveWorktree,
                &request.operation_id,
                &request.request_sha256,
            )
        });
        let agent_id = agent.id.clone();
        self.progress(
            agent,
            AgentWorkStep::PublishDeleted,
            now,
            evidence,
            Vec::new(),
            Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree: worktree.clone(),
            }),
            Some(SandboxEventKind::WorktreeRemoved {
                workspace_id: worktree.workspace_id,
                worktree_id: worktree.id,
            }),
        )
        .await
        .map_err(|error| {
            tracing::debug!(agent_id = %agent_id, %error, "worktree removal publication failed");
            error
        })
    }

    async fn invalid_cleanup(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.handle_failure(
            agent,
            WorkerFailure::retryable(
                "agent_cleanup_state_invalid",
                "agent cleanup state does not match its durable work step",
            ),
            now,
        )
        .await
    }
}

fn close_request(agent: &Agent) -> AgentCloseRequest {
    AgentCloseRequest::new(
        format!("agent:{}:close:v1", agent.id),
        AgentSlot::new(agent.id.to_string()),
    )
}

fn has_receipt(agent: &Agent, step: AgentWorkStep) -> bool {
    agent
        .work
        .receipts
        .iter()
        .any(|receipt| receipt.step == step)
}

fn cleanup_failure(error: layer3_adapter::AdapterError) -> WorkerFailure {
    WorkerFailure::retryable("agent_cleanup_unavailable", error.to_string())
}
