use std::time::SystemTime;

use layer2_core::sandbox::store::{AgentWorkPublicationParents, SandboxStoreError};
use layer2_core::sandbox::{
    Agent, AgentStatus, AgentWorkStep, FirstMessageDelivery, SandboxStatus, WorkspaceStatus,
};

use super::startup_support::{live_target, registry_failure};
use super::support::{claimed_fence, AgentContext, WorkerFailure};
use super::AgentWorker;

const MAX_PUBLICATION_RETRIES: usize = 8;

impl super::AgentWorkerConfig {
    pub(crate) fn with_parent_not_ready_pacing(mut self, enabled: bool) -> Self {
        self.parent_not_ready_pacing_enabled = enabled;
        self
    }
}

impl AgentWorker {
    pub(super) async fn publish_connection(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let context = match self.connection_context(&agent).await {
            Ok(context) => context,
            Err(failure) => {
                return self
                    .handle_failure(agent, failure, now + started.elapsed())
                    .await
            }
        };
        let target = match live_target(&agent) {
            Ok(target) => target,
            Err(failure) => {
                return self
                    .handle_failure(agent, failure, now + started.elapsed())
                    .await
            }
        };
        let fence = claimed_fence(&agent, &self.config.holder)?;
        if let Err(failure) = self.recorded_open_request(&agent, &context).await {
            return self
                .handle_failure(agent, failure, now + started.elapsed())
                .await;
        }
        // The initial open has already negotiated this exact conversation. Keep
        // that unpublished stream instead of opening it again. Settings replay
        // and missing local state still require the full reconnect protocol.
        let staged = if agent.conversation.settings.is_none() {
            self.dependencies.registry.staged_attachment(&target, fence)
        } else {
            Ok(None)
        };
        let attachment = match staged {
            Ok(Some(attachment)) => attachment,
            Ok(None) => match self.reattach(&agent, &context, &target).await {
                Ok(attachment) => attachment,
                Err(failure) => {
                    return self
                        .handle_failure(agent, failure, now + started.elapsed())
                        .await
                }
            },
            Err(error) => {
                return self
                    .handle_failure(agent, registry_failure(error), now + started.elapsed())
                    .await;
            }
        };
        if let Err(error) = self.dependencies.registry.publish_and_activate(&attachment) {
            self.dependencies.registry.rollback(&attachment);
            return self
                .handle_failure(agent, registry_failure(error), now + started.elapsed())
                .await;
        }
        self.finish_connection_publication(
            agent,
            &attachment,
            Some(context.publication_parents()),
            now + started.elapsed(),
        )
        .await
    }

    pub(super) async fn connection_context(
        &self,
        agent: &Agent,
    ) -> Result<AgentContext, WorkerFailure> {
        if !matches!(
            agent.status,
            AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
        ) || agent.requested_stop_reason.is_some()
        {
            return Err(WorkerFailure::terminal(
                "agent_connection_state_invalid",
                "connection publication requires an attachable live agent without stop intent",
            ));
        }
        let context = self.context(agent).await?;
        if (context.sandbox.status != SandboxStatus::Active
            && !context.sandbox.restored_connection_is_ready(agent))
            || context.workspace.status != WorkspaceStatus::Ready
            || context.worktree.status != layer2_core::sandbox::WorktreeStatus::Ready
        {
            return Err(WorkerFailure::retryable(
                "agent_parent_not_ready",
                "the sandbox, workspace, or worktree is not ready for connection publication",
            ));
        }
        Ok(context)
    }

    pub(super) async fn try_finish_connection_publication(
        &self,
        mut agent: Agent,
        attachment: &crate::sandbox::live_agents::LiveAgentAttachment,
        parents: Option<AgentWorkPublicationParents>,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let started = tokio::time::Instant::now();
        for _ in 0..MAX_PUBLICATION_RETRIES {
            // Reattachment may confirm settings/context after the original work
            // claim. A refreshed revision must never be published at an older
            // timestamp, including on later optimistic retries.
            let durable_at = layer2_core::seconds_from_rfc3339(&agent.updated_at)
                .map(|seconds| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
                .ok_or_else(|| SandboxStoreError::Corrupt("invalid agent update time".into()))?;
            let at = (now + started.elapsed()).max(durable_at);
            let next =
                if agent
                    .conversation
                    .first_message
                    .as_ref()
                    .is_some_and(|message| message.state == FirstMessageDelivery::Queued)
                {
                    AgentWorkStep::DispatchFirstMessage
                } else if agent.work.live_skills.last().is_some_and(|s| {
                    s.status == layer2_core::sandbox::AgentSkillInstallStatus::Pending
                }) {
                    AgentWorkStep::InstallSkill
                } else {
                    AgentWorkStep::None
                };
            let mutation =
                self.progress_mutation(agent.clone(), next, at, None, Vec::new(), None, None)?;
            let result = if let Some(parents) = parents {
                self.dependencies
                    .store
                    .commit_agent_connection_work(mutation, parents)
                    .await
            } else {
                self.dependencies.store.commit_agent_work(mutation).await
            };
            match result {
                Ok(()) => {
                    self.dependencies
                        .registry
                        .recover_lost_publication(attachment)
                        .await;
                    return Ok(());
                }
                Err(SandboxStoreError::Conflict) => {
                    let Some(current) = self.dependencies.store.get_agent(&agent.id).await? else {
                        return Err(SandboxStoreError::NotFound);
                    };
                    if !matches!(
                        current.status,
                        AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
                    ) || current.work.step != AgentWorkStep::PublishConnection
                        || !current.work.owns(&self.config.holder, fence)
                        || live_target(&current).ok() != live_target(&agent).ok()
                    {
                        return Err(SandboxStoreError::Conflict);
                    }
                    agent = current;
                }
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}
