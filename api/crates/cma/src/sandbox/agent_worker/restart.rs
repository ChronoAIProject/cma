//! Reopen a retained conversation under its admitted runtime and current work lease.
use super::{
    support::{claimed_fence, runtime_options, AgentContext, PublicationParents, WorkerFailure},
    AgentWorker,
};
use crate::sandbox::worker::lifecycle::reopen_output;
use layer2_core::sandbox::{
    store::{
        owner_restart_ready, AgentReopenGuard, AgentRestartDispatch, AgentWorkPublication,
        OwnerRestartOutputFence, SandboxMutationGuard, SandboxStoreError,
    },
    Agent, SandboxStatus, WorkspaceStatus, WorktreeStatus,
};
use layer2_core::{rfc3339_at, seconds_from_rfc3339};
use layer3_adapter::{AgentOpenRequest, AgentProcessAction, AgentRestartRuntime};
use std::time::SystemTime;

impl AgentWorker {
    pub(super) async fn restart_conversation(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let context = match self.context(&agent).await {
            Ok(context) => context,
            Err(_) => return self.retry_restart(agent, now + started.elapsed()).await,
        };
        let work = self.dependencies.store.agent_restart_work(&agent).await?;
        let acceptance = work.acceptance.clone();
        if !self
            .runtime_is_pinned(&context, &acceptance.source.runtime)
            .await
        {
            return self.retry_restart(agent, now + started.elapsed()).await;
        }

        let open = match &work.dispatched_open {
            Some(open) => open.clone(),
            None => {
                let mut request = AgentOpenRequest::new(
                    AgentProcessAction::Open
                        .operation_id(agent.id.as_str(), acceptance.next_incarnation),
                    runtime_options(&agent, &context.sandbox, &context.worktree),
                    Some(acceptance.source.conversation_id.clone()),
                );
                request.restart_runtime = Some(acceptance.source.runtime.clone());
                request
            }
        };
        let claimed = self
            .dependencies
            .store
            .prepare_agent_restart_dispatch(AgentRestartDispatch {
                claimed: agent,
                work,
                open: open.clone(),
                dispatched_at: rfc3339_at(now + started.elapsed()),
            })
            .await?;

        let fence = claimed_fence(&claimed, &self.config.holder)?;
        let lease = claimed
            .work
            .lease
            .clone()
            .ok_or(SandboxStoreError::Conflict)?;
        let expires = seconds_from_rfc3339(&lease.expires_at).ok_or(SandboxStoreError::Conflict)?;
        let send_at = seconds_from_rfc3339(&rfc3339_at(now + started.elapsed()))
            .ok_or(SandboxStoreError::Conflict)?;
        let remaining = expires.saturating_sub(send_at);
        if remaining == 0 {
            return Err(SandboxStoreError::Conflict);
        }

        let runtime_generation = claimed
            .conversation
            .runtime_generation
            .and_then(|prior| prior.checked_add(1))
            .ok_or(SandboxStoreError::Conflict)?;
        let guard = AgentReopenGuard {
            sandbox: SandboxMutationGuard::capture(&context.sandbox),
            agent_fence: claimed.work.next_fence,
            native_conversation_id: acceptance.source.conversation_id.clone(),
            runtime_generation,
            owner_restart: Some(OwnerRestartOutputFence {
                agent_id: claimed.id.clone(),
                restart_operation: acceptance.operation_id,
                open_operation: open.operation_id.clone(),
                open_request_sha256: open.request_sha256.clone(),
                lease: lease.clone(),
            }),
        };
        let (updates, mut receiver) = tokio::sync::mpsc::channel(256);
        let handshake = async {
            let channel = self
                .dependencies
                .adapter
                .reconnect_agent(
                    &context.runtime,
                    fence,
                    &open,
                    &guard.native_conversation_id,
                    updates,
                )
                .await?;
            if channel.conversation_id.as_deref() != Some(&guard.native_conversation_id) {
                return Err(layer3_adapter::AdapterError::Rejected(
                    "restart returned another conversation".into(),
                ));
            }
            if let Some(settings) = &claimed.conversation.settings {
                channel.input.send(settings.runtime_input()).await?;
            }
            Ok(channel)
        };
        let timeout = self
            .config
            .open_timeout
            .min(std::time::Duration::from_secs(remaining));
        let channel = tokio::time::timeout(
            timeout,
            reopen_output::drain(
                handshake,
                &mut receiver,
                self.dependencies.updates.as_ref(),
                &claimed.id,
                &guard,
                true,
            ),
        )
        .await;
        // The output recorder can advance the Agent revision while retaining this lease.
        let current = self
            .dependencies
            .store
            .get_agent(&claimed.id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if current.work.lease.as_ref() != Some(&lease) {
            return Err(SandboxStoreError::Conflict);
        }
        let Ok(Ok(channel)) = channel else {
            return self.retry_restart(current, now + started.elapsed()).await;
        };
        let opened = self.dependencies.store.agent_restart_work(&current).await?;
        if opened.dispatched_open.as_ref() != Some(&open)
            || !self
                .runtime_is_pinned(&context, &opened.acceptance.source.runtime)
                .await
        {
            return self.retry_restart(current, now + started.elapsed()).await;
        }
        let attachment = match self.attach(
            &current,
            guard.native_conversation_id,
            runtime_generation,
            channel.input,
            receiver,
        ) {
            Ok(attachment) => attachment,
            Err(_) => return self.retry_restart(current, now + started.elapsed()).await,
        };
        // The native Open and retained output are already confirmed. Rebuild
        // only the Ready publication if compatible parent revisions moved.
        let parents = PublicationParents {
            sandbox: context.sandbox,
            workspace: context.workspace,
            worktree: context.worktree,
        };
        match self
            .try_publish_lifecycle_work(
                current.clone(),
                Some(parents),
                now + started.elapsed(),
                |agent, at, parents| async move {
                    let proposal =
                        owner_restart_ready(&agent, &self.config.holder, fence, rfc3339_at(at))?;
                    self.commit_lifecycle_publication(proposal, parents).await
                },
            )
            .await
        {
            Ok(AgentWorkPublication::Published) => {
                if self
                    .dependencies
                    .registry
                    .publish_and_activate(&attachment)
                    .is_err()
                {
                    self.dependencies
                        .registry
                        .recover_lost_publication(&attachment)
                        .await;
                }
                Ok(())
            }
            Ok(AgentWorkPublication::KnownAborted) => {
                self.dependencies.registry.rollback(&attachment);
                self.retry_restart(current, now + started.elapsed()).await
            }
            Err(error) => {
                self.dependencies.registry.rollback(&attachment);
                Err(error)
            }
        }
    }

    async fn runtime_is_pinned(&self, context: &AgentContext, pin: &AgentRestartRuntime) -> bool {
        context.sandbox.status == SandboxStatus::Active
            && context.sandbox.lifecycle.operation.is_none()
            && context.workspace.status == WorkspaceStatus::Ready
            && context.worktree.status == WorktreeStatus::Ready
            && self
                .dependencies
                .adapter
                .runtime_status(&context.runtime)
                .await
                .is_ok_and(|status| status.restart_runtime().as_ref().ok() == Some(pin))
    }

    async fn retry_restart(&self, agent: Agent, now: SystemTime) -> Result<(), SandboxStoreError> {
        self.handle_failure(
            agent,
            WorkerFailure::retryable(
                "agent_restart_pending",
                "the admitted Agent restart is awaiting exact runtime confirmation",
            ),
            now,
        )
        .await
    }
}
