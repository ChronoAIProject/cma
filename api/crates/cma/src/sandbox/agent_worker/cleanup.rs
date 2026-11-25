use std::time::SystemTime;

use layer2_core::sandbox::store::{
    retired_sandbox_cleanup_is_absent, retiring_sandbox_process_is_absent, SandboxStoreError,
    VersionedWorktree,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkStep, SandboxEventKind, WorktreeStatus,
};
use layer3_adapter::{AgentHomeRemoveRequest, AgentSlot, WorktreeRef};

use super::startup_support::has_startup_evidence;
use super::support::{
    claimed_fence, cleanup_failure, request_evidence, validate_lineage, WorkerFailure,
};
use super::AgentWorker;

#[path = "cleanup_retired.rs"]
mod retired;
pub(super) use retired::CleanupPreflightError;

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
        let started = tokio::time::Instant::now();
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
                        .handle_failure(agent, cleanup_failure(error), now + started.elapsed())
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
        if sandbox.runtime.is_none() {
            return self.handle_failure(agent, WorkerFailure::retryable(
                "agent_stop_runtime_verification_required",
                "The runtime binding is absent but process absence is not proved. Retire the Sandbox to reconcile its provider ownership and retained home; if retirement is already pending, restore provider access or repair the retained ownership evidence. No process has been reported stopped.",
            ), now).await;
        }
        let context = match self.cleanup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => {
                return self
                    .handle_failure(agent, failure, now + started.elapsed())
                    .await
            }
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
        let request = match agent.close_process_request() {
            Ok(request) => request,
            Err(_) => return self.invalid_cleanup(agent, now).await,
        };
        let authority_fence = claimed_fence(&agent, &self.config.holder)?;
        let (closed, failure) = layer3_adapter::receiver_failure::observe(
            self.dependencies
                .adapter
                .close_agent(&context.runtime, authority_fence, &request),
        )
        .await;
        if let (Some(store), Some(receiver), Some(binding)) = (
            &self.config.retirement_reporter,
            failure.and_then(|f| f.into_receiver()),
            &context.sandbox.runtime,
        ) {
            if receiver.owner.agent_id == agent.id.as_str()
                && context.sandbox.status == layer2_core::sandbox::SandboxStatus::Retiring
                && context.sandbox.work.step == layer2_core::sandbox::SandboxWorkStep::StopAgents
            {
                let observation = crate::store::native_retirement_action::RetirementObservation {
                    owner: context.sandbox.owner.clone(),
                    binding: binding.clone(),
                    receiver,
                };
                if let Err(error) = store.note(&context.sandbox, &observation).await {
                    tracing::debug!(sandbox_id = %context.sandbox.id,
                        retryable = matches!(error, SandboxStoreError::Unavailable(_)),
                        "retirement receiver observation was not recorded");
                }
            }
        }
        match closed {
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
                self.handle_failure(agent, cleanup_failure(error), now + started.elapsed())
                    .await
            }
        }
    }

    pub(super) async fn ensure_process_closed(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        if !matches!(agent.status, AgentStatus::Stopped | AgentStatus::Failed) {
            return self.invalid_cleanup(agent, now).await;
        }
        let fence = claimed_fence(&agent, &self.config.holder)?;
        self.dependencies
            .registry
            .fence(&agent.id, agent.conversation.runtime_generation, fence);
        let context = match self.cleanup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => {
                return self
                    .handle_failure(agent, failure, now + started.elapsed())
                    .await
            }
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

        let open_exists = has_startup_evidence(&agent, AgentWorkStep::OpenConversation);
        let previous_close = agent
            .work
            .receipts
            .iter()
            .find(|receipt| receipt.step == AgentWorkStep::CloseProcess);
        if open_exists
            && previous_close.is_some_and(|receipt| {
                !agent
                    .close_process_request()
                    .is_ok_and(|request| request.operation_id == receipt.operation_id)
            })
        {
            return self.invalid_cleanup(agent, now).await;
        }
        if previous_close.is_some() || !open_exists {
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

        let request = match agent.close_process_request() {
            Ok(request) => request,
            Err(_) => return self.invalid_cleanup(agent, now).await,
        };
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
                self.handle_failure(agent, cleanup_failure(error), now + started.elapsed())
                    .await
            }
        }
    }

    pub(super) async fn remove_home(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let context = match self.cleanup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => {
                return self
                    .handle_failure(agent, failure, now + started.elapsed())
                    .await
            }
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
                self.handle_failure(agent, cleanup_failure(error), now + started.elapsed())
                    .await
            }
        }
    }

    pub(super) async fn remove_worktree(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let context = match self.cleanup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => {
                return self
                    .handle_failure(agent, failure, now + started.elapsed())
                    .await
            }
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
                    .handle_failure(agent, cleanup_failure(error), now + started.elapsed())
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

    pub(super) async fn invalid_cleanup(
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
