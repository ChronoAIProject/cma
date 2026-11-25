use std::time::SystemTime;

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentWorkStep};
use layer3_adapter::{AgentHomeRequest, AgentOpenRequest, ConversationChannel, WorktreeRequest};
use tokio::sync::mpsc;

use super::startup_support::{
    live_target, registry_failure, valid_conversation_id, valid_worktree_receipt,
};
use super::support::{
    adapter_failure, claimed_fence, request_evidence, runtime_capabilities, runtime_options,
    skill_failure, validate_runtime_skills, validate_skills, AgentContext, WorkerFailure,
};
use super::AgentWorker;
use crate::sandbox::live_agents::{LiveAgentAttachment, LiveAgentTarget};

const UPDATE_BUFFER: usize = 256;

#[path = "startup_attachment.rs"]
mod attachment;
pub(super) use attachment::validated_open_request;

impl AgentWorker {
    pub(super) async fn add_worktree(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let context = match self.startup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => {
                return self
                    .handle_failure(agent, failure, now + started.elapsed())
                    .await
            }
        };
        if self
            .dependencies
            .store
            .prepared_agent_replay_matches(
                &agent,
                &context.sandbox,
                &context.workspace,
                &context.worktree,
            )
            .await?
        {
            return self
                .replay_and_open_prepared(agent, context, now + started.elapsed())
                .await;
        }
        let request = WorktreeRequest::new(
            format!(
                "worktree:{}:{}:add:v1",
                context.workspace.name, context.worktree.name
            ),
            context.workspace.name.clone(),
            context.worktree.name.clone(),
            context.worktree.base_commit.clone(),
        );
        let evidence = request_evidence(
            AgentWorkStep::AddWorktree,
            &request.operation_id,
            &request.request_sha256,
        );
        let agent = self
            .prepare_side_effect(
                agent,
                &evidence,
                None,
                &context.sandbox,
                now + started.elapsed(),
            )
            .await?;
        let authority_fence = claimed_fence(&agent, &self.config.holder)?;
        let result = self
            .dependencies
            .adapter
            .add_worktree(&context.runtime, authority_fence, &request)
            .await;
        let now = now + started.elapsed();
        match result {
            Ok(receipt) if valid_worktree_receipt(&context, &request, &receipt) => {
                self.startup_progress(
                    agent,
                    AgentWorkStep::MaterializeHome,
                    now,
                    evidence,
                    None,
                    &context.sandbox,
                )
                .await
            }
            Ok(_) => {
                self.handle_context_failure(
                    &context,
                    agent,
                    WorkerFailure::terminal(
                        "agent_runtime_receipt_invalid",
                        "runtime returned invalid worktree evidence",
                    ),
                    now,
                )
                .await
            }
            Err(error) => {
                self.handle_context_failure(&context, agent, adapter_failure(error), now)
                    .await
            }
        }
    }

    pub(super) async fn materialize_home(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let context = match self.startup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => {
                return self
                    .handle_failure(agent, failure, now + started.elapsed())
                    .await
            }
        };
        let material = match &agent.work.resolved_skills {
            Some(material) => material.clone(),
            None if agent
                .work
                .cleanup_debt
                .iter()
                .any(|debt| debt.step == AgentWorkStep::MaterializeHome)
                || agent
                    .work
                    .receipts
                    .iter()
                    .any(|receipt| receipt.step == AgentWorkStep::MaterializeHome) =>
            {
                return self
                    .handle_context_failure(
                        &context,
                        agent,
                        WorkerFailure::terminal(
                            "agent_skill_material_missing",
                            "durable home intent has no immutable skill selection",
                        ),
                        now,
                    )
                    .await;
            }
            None => match self
                .dependencies
                .skills
                .resolve(&context.sandbox, &agent.profile.content.skills)
                .await
            {
                Ok(material) => material,
                Err(error) => {
                    return self
                        .handle_context_failure(&context, agent, skill_failure(error), now)
                        .await
                }
            },
        };
        if let Err(failure) = validate_skills(&agent.profile.content.skills, &material) {
            return self
                .handle_context_failure(&context, agent, failure, now)
                .await;
        }
        let skills = match self.dependencies.skills.runtime_skills(&material) {
            Ok(skills) => skills,
            Err(error) => {
                return self
                    .handle_context_failure(&context, agent, skill_failure(error), now)
                    .await
            }
        };
        if let Err(failure) = validate_runtime_skills(&material, &skills) {
            return self
                .handle_context_failure(&context, agent, failure, now)
                .await;
        }
        let request = AgentHomeRequest::new(
            format!("agent:{}:home:create:v1", agent.id),
            runtime_options(&agent, &context.sandbox, &context.worktree),
            agent.profile.content.system_prompt.clone(),
            skills,
        );
        let evidence = request_evidence(
            AgentWorkStep::MaterializeHome,
            &request.operation_id,
            &request.request_sha256,
        );
        let agent = self
            .prepare_side_effect(
                agent,
                &evidence,
                Some(material),
                &context.sandbox,
                now + started.elapsed(),
            )
            .await?;
        let authority_fence = claimed_fence(&agent, &self.config.holder)?;
        let result = self
            .dependencies
            .adapter
            .materialize_agent_home(&context.runtime, authority_fence, &request)
            .await;
        let now = now + started.elapsed();
        match result {
            Ok(()) => {
                self.startup_progress(
                    agent,
                    AgentWorkStep::OpenConversation,
                    now,
                    evidence,
                    None,
                    &context.sandbox,
                )
                .await
            }
            Err(error) => {
                self.handle_context_failure(&context, agent, adapter_failure(error), now)
                    .await
            }
        }
    }

    pub(super) async fn open_conversation(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        // The context path owns the open and publication state machines.
        Box::pin(self.open_conversation_with_context(agent, None, now)).await
    }

    pub(super) async fn open_conversation_with_context(
        &self,
        agent: Agent,
        prepared: Option<AgentContext>,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let attempt_now = now;
        let mut timing = super::startup_timing::StartupTiming::new(&agent);
        let context = match match prepared {
            Some(context) => self.prepared_open_context(&agent, context).await,
            None => self.startup_context(&agent).await,
        } {
            Ok(context) => context,
            Err(failure) => {
                return self
                    .handle_failure(agent, failure, now + started.elapsed())
                    .await
            }
        };
        timing.completed("open_context");
        if agent.conversation.conversation_id.is_some()
            || agent.conversation.runtime_generation.is_some()
        {
            return self
                .handle_context_failure(
                    &context,
                    agent,
                    WorkerFailure::terminal(
                        "agent_conversation_state_invalid",
                        "an unopened starting agent already has runtime identity",
                    ),
                    now,
                )
                .await;
        }
        let request = AgentOpenRequest::new(
            format!("agent:{}:open:v1", agent.id),
            runtime_options(&agent, &context.sandbox, &context.worktree),
            None,
        );
        let evidence = request_evidence(
            AgentWorkStep::OpenConversation,
            &request.operation_id,
            &request.request_sha256,
        );
        let mut agent = self
            .prepare_side_effect(
                agent,
                &evidence,
                None,
                &context.sandbox,
                now + started.elapsed(),
            )
            .await?;
        timing.completed("open_intent");
        let authority_fence = claimed_fence(&agent, &self.config.holder)?;
        let opened = self
            .open_starting(
                &mut agent,
                &context,
                authority_fence,
                &request,
                now + started.elapsed(),
            )
            .await;
        timing.completed("open_runtime");
        let startup_timing = timing.finish();
        let agent_id = agent.id.clone();
        self.retain_startup_timing(&agent_id, startup_timing);
        let now = now + started.elapsed();
        let (channel, updates) = match opened {
            Ok(opened) => opened,
            Err(failure) => {
                return self
                    .handle_context_failure(&context, agent, failure, now)
                    .await
            }
        };
        let conversation_id = match valid_conversation_id(channel.conversation_id) {
            Ok(id) => id,
            Err(failure) => {
                return self
                    .handle_context_failure(&context, agent, failure, now)
                    .await
            }
        };
        let generation = 1;
        let attachment = match self.attach(
            &agent,
            conversation_id.clone(),
            generation,
            channel.input,
            updates,
        ) {
            Ok(attachment) => attachment,
            Err(failure) => {
                return self
                    .handle_context_failure(&context, agent, failure, now)
                    .await
            }
        };
        let result = async {
            let published = self
                .publish_prepared_open_ready(&agent, &context, &evidence, &attachment, now)
                .await;
            match published {
                Ok(true) => {
                    tracing::info!(agent_id = %agent.id, "Prepared Agent Open and Ready published atomically");
                    return self
                        .publish_prepared_connection(
                            &agent,
                            &context,
                            &attachment,
                            attempt_now + started.elapsed(),
                        )
                        .await;
                }
                Err(SandboxStoreError::Conflict) => {
                    tracing::info!(agent_id = %agent.id, outcome = "conflict", "Prepared Agent publication selected ordinary readiness checks");
                    return self
                        .retain_completed_open_after_conflict(
                            &agent,
                            &context.sandbox,
                            &request,
                            &attachment,
                            attempt_now + started.elapsed(),
                        )
                        .await;
                }
                Err(error) => return Err(error),
                Ok(false) => {
                    tracing::info!(agent_id = %agent.id, outcome = "ineligible", "Prepared Agent publication selected ordinary readiness checks");
                }
            }
            self.startup_progress(
                agent,
                AgentWorkStep::PublishReady,
                now,
                evidence,
                Some((conversation_id, generation)),
                &context.sandbox,
            )
            .await
        }
        .await;
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                self.dependencies.registry.rollback(&attachment);
                Err(error)
            }
        }
    }

    pub(super) async fn publish_ready(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let context = match self.startup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => {
                return self
                    .handle_failure(agent, failure, now + started.elapsed())
                    .await
            }
        };
        if let Err(detail) = layer2_core::sandbox::desktop_creation::prepare_desktop(
            self.dependencies.adapter.as_ref(),
            &agent,
            &context.sandbox,
        )
        .await
        {
            return self
                .handle_context_failure(
                    &context,
                    agent,
                    WorkerFailure::retryable("desktop_preparation_unavailable", detail),
                    now + started.elapsed(),
                )
                .await;
        }
        let target = match live_target(&agent) {
            Ok(target) => target,
            Err(failure) => {
                return self
                    .handle_context_failure(&context, agent, failure, now + started.elapsed())
                    .await
            }
        };
        let authority_fence = claimed_fence(&agent, &self.config.holder)?;
        let attachment = match self
            .dependencies
            .registry
            .attachment(&target, authority_fence)
        {
            Ok(Some(attachment)) => attachment,
            Ok(None) => match self.reattach(&agent, &context, &target).await {
                Ok(attachment) => attachment,
                Err(failure) => {
                    return self
                        .handle_context_failure(&context, agent, failure, now + started.elapsed())
                        .await
                }
            },
            Err(error) => {
                return self
                    .handle_context_failure(
                        &context,
                        agent,
                        registry_failure(error),
                        now + started.elapsed(),
                    )
                    .await;
            }
        };
        let result = self
            .retry_startup_publication(
                agent,
                &context.sandbox,
                now + started.elapsed(),
                |current, at| self.publish_ready_once(current, &context, &target, at),
            )
            .await;
        if result.is_err() {
            self.dependencies.registry.rollback(&attachment);
        }
        result
    }
}
