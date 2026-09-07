use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{AgentWorkMutation, SandboxStoreError, VersionedWorktree};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkStep, SandboxEventKind, WorktreeStatus,
};
use layer3_adapter::{AgentHomeRequest, AgentOpenRequest, ConversationChannel, WorktreeRequest};
use tokio::sync::mpsc;

use super::startup_support::{
    live_target, registry_failure, valid_conversation_id, valid_worktree_receipt,
};
use super::support::{
    adapter_failure, bump_log_sequence, claimed_fence, request_evidence, runtime_capabilities,
    runtime_options, skill_failure, validate_runtime_skills, validate_skills, AgentContext,
    WorkerFailure,
};
use super::AgentWorker;
use crate::sandbox::live_agents::{LiveAgentAttachment, LiveAgentTarget};

const UPDATE_BUFFER: usize = 256;

impl AgentWorker {
    pub(super) async fn add_worktree(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let context = match self.startup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
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
            .prepare_side_effect(agent, &evidence, None, now)
            .await?;
        let authority_fence = claimed_fence(&agent, &self.config.holder)?;
        match self
            .dependencies
            .adapter
            .add_worktree(&context.runtime, authority_fence, &request)
            .await
        {
            Ok(receipt) if valid_worktree_receipt(&context, &request, &receipt) => {
                self.progress(
                    agent,
                    AgentWorkStep::MaterializeHome,
                    now,
                    Some(evidence),
                    Vec::new(),
                    None,
                    None,
                )
                .await
            }
            Ok(_) => {
                self.handle_failure(
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
                self.handle_failure(agent, adapter_failure(error), now)
                    .await
            }
        }
    }

    pub(super) async fn materialize_home(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let context = match self.startup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
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
                    .handle_failure(
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
                Err(error) => return self.handle_failure(agent, skill_failure(error), now).await,
            },
        };
        if let Err(failure) = validate_skills(&agent.profile.content.skills, &material) {
            return self.handle_failure(agent, failure, now).await;
        }
        let skills = match self.dependencies.skills.runtime_skills(&material) {
            Ok(skills) => skills,
            Err(error) => return self.handle_failure(agent, skill_failure(error), now).await,
        };
        if let Err(failure) = validate_runtime_skills(&material, &skills) {
            return self.handle_failure(agent, failure, now).await;
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
            .prepare_side_effect(agent, &evidence, Some(material), now)
            .await?;
        let authority_fence = claimed_fence(&agent, &self.config.holder)?;
        match self
            .dependencies
            .adapter
            .materialize_agent_home(&context.runtime, authority_fence, &request)
            .await
        {
            Ok(()) => {
                self.progress(
                    agent,
                    AgentWorkStep::OpenConversation,
                    now,
                    Some(evidence),
                    Vec::new(),
                    None,
                    None,
                )
                .await
            }
            Err(error) => {
                self.handle_failure(agent, adapter_failure(error), now)
                    .await
            }
        }
    }

    pub(super) async fn open_conversation(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let context = match self.startup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
        if agent.conversation.conversation_id.is_some()
            || agent.conversation.runtime_generation.is_some()
        {
            return self
                .handle_failure(
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
            .prepare_side_effect(agent, &evidence, None, now)
            .await?;
        let authority_fence = claimed_fence(&agent, &self.config.holder)?;
        let opening_started = tokio::time::Instant::now();
        let opened = self
            .open_starting(&mut agent, &context, authority_fence, &request, now)
            .await;
        let now = now + opening_started.elapsed();
        let (channel, updates) = match opened {
            Ok(opened) => opened,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
        let conversation_id = match valid_conversation_id(channel.conversation_id) {
            Ok(id) => id,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
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
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
        agent.conversation.conversation_id = Some(conversation_id);
        agent.conversation.runtime_generation = Some(generation);
        let result = self
            .progress(
                agent,
                AgentWorkStep::PublishReady,
                now,
                Some(evidence),
                Vec::new(),
                None,
                None,
            )
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
        mut agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let context = match self.startup_context(&agent).await {
            Ok(context) => context,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
        let target = match live_target(&agent) {
            Ok(target) => target,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
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
                Err(failure) => return self.handle_failure(agent, failure, now).await,
            },
            Err(error) => {
                return self
                    .handle_failure(agent, registry_failure(error), now)
                    .await;
            }
        };
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let stamp = rfc3339_at(now);
        agent
            .work
            .advance(
                &self.config.holder,
                fence,
                AgentWorkStep::PublishConnection,
                stamp.clone(),
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        agent
            .transition(AgentStatus::Ready, stamp.clone(), None)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let conversation_id = target.conversation_id;
        let runtime_generation = target.runtime_generation;
        let logs = vec![
            AgentLogKind::ConversationOpened {
                conversation_id,
                runtime_generation,
            },
            AgentLogKind::StatusChanged {
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            },
        ];
        bump_log_sequence(&mut agent, logs.len())?;
        let mut worktree = context.worktree;
        let expected_worktree_revision = worktree.revision;
        worktree
            .transition(WorktreeStatus::Ready)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let result = self
            .dependencies
            .store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: expected,
                holder: self.config.holder.clone(),
                fence,
                agent: agent.clone(),
                worktree: Some(VersionedWorktree {
                    expected_revision: expected_worktree_revision,
                    worktree,
                }),
                appended_at: stamp,
                sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                    agent_id: agent.id.clone(),
                    from: AgentStatus::Starting,
                    to: AgentStatus::Ready,
                }),
                agent_logs: logs,
            })
            .await;
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                self.dependencies.registry.rollback(&attachment);
                Err(error)
            }
        }
    }

    pub(super) async fn reattach(
        &self,
        agent: &Agent,
        context: &AgentContext,
        target: &LiveAgentTarget,
    ) -> Result<LiveAgentAttachment, WorkerFailure> {
        let connection_fence = agent
            .work
            .lease
            .as_ref()
            .filter(|lease| lease.holder == self.config.holder)
            .map(|lease| lease.fence)
            .ok_or_else(|| {
                WorkerFailure::retryable(
                    "agent_lifecycle_fence_lost",
                    "agent lifecycle lease is no longer held",
                )
            })?;
        let request = AgentOpenRequest::new(
            format!("agent:{}:open:v1", agent.id),
            runtime_options(agent, &context.sandbox, &context.worktree),
            None,
        );
        if agent
            .work
            .receipts
            .iter()
            .find(|receipt| receipt.step == AgentWorkStep::OpenConversation)
            .filter(|receipt| {
                receipt.operation_id == request.operation_id
                    && receipt.request_sha256 == request.request_sha256
            })
            .is_none()
        {
            return Err(WorkerFailure::terminal(
                "agent_conversation_receipt_invalid",
                "the durable conversation-open receipt does not match replay",
            ));
        }
        let (channel, updates) = self
            .reconnect(context, connection_fence, &request, &target.conversation_id)
            .await?;
        let conversation_id = valid_conversation_id(channel.conversation_id)?;
        if conversation_id != target.conversation_id {
            return Err(WorkerFailure::terminal(
                "agent_conversation_identity_mismatch",
                "runtime replay returned another conversation",
            ));
        }
        self.attach(
            agent,
            conversation_id,
            target.runtime_generation,
            channel.input,
            updates,
        )
    }

    async fn reconnect(
        &self,
        context: &AgentContext,
        authority_fence: u64,
        request: &AgentOpenRequest,
        conversation_id: &str,
    ) -> Result<
        (
            ConversationChannel,
            mpsc::Receiver<layer3_adapter::ConversationUpdate>,
        ),
        WorkerFailure,
    > {
        let (updates, receiver) = mpsc::channel(UPDATE_BUFFER);
        match tokio::time::timeout(
            self.config.open_timeout,
            self.dependencies.adapter.reconnect_agent(
                &context.runtime,
                authority_fence,
                request,
                conversation_id,
                updates,
            ),
        )
        .await
        {
            Ok(Ok(channel)) => Ok((channel, receiver)),
            Ok(Err(error)) => Err(adapter_failure(error)),
            Err(_) => Err(WorkerFailure::retryable(
                "agent_conversation_open_timeout",
                "conversation did not become ready within 120 seconds",
            )),
        }
    }

    fn attach(
        &self,
        agent: &Agent,
        conversation_id: String,
        runtime_generation: u64,
        input: layer3_adapter::ConversationInputSender,
        updates: mpsc::Receiver<layer3_adapter::ConversationUpdate>,
    ) -> Result<LiveAgentAttachment, WorkerFailure> {
        let fence = agent
            .work
            .lease
            .as_ref()
            .filter(|lease| lease.holder == self.config.holder)
            .map(|lease| lease.fence)
            .ok_or_else(|| {
                WorkerFailure::retryable(
                    "agent_lifecycle_fence_lost",
                    "agent lifecycle lease is no longer held",
                )
            })?;
        let offered = runtime_capabilities(self.dependencies.adapter.capabilities());
        offered
            .require(agent.capabilities.iter())
            .map_err(|capability| {
                WorkerFailure::terminal(
                    "agent_capability_grant_unavailable",
                    format!("replacement runtime does not provide {capability:?}"),
                )
            })?;
        self.dependencies
            .registry
            .attach(
                agent.id.clone(),
                conversation_id,
                runtime_generation,
                fence,
                agent.capabilities.clone(),
                input,
                updates,
                self.dependencies.updates.clone(),
            )
            .map_err(registry_failure)
    }
}
