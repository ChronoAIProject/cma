use std::future::Future;
use std::time::{Duration, SystemTime};

use crate::sandbox::live_agents::LiveAgentTarget;
use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{AgentWorkMutation, SandboxStoreError, VersionedWorktree};
use layer2_core::sandbox::{
    Agent, AgentStatus, AgentWorkStep, FirstMessageRecord, Sandbox, SandboxStatus,
};
use layer2_core::sandbox::{AgentLogKind, SandboxEventKind, WorktreeStatus};
use layer2_core::seconds_from_rfc3339;

use super::support::{bump_log_sequence, claimed_fence, AgentContext, RequestEvidence};
use super::AgentWorker;

const MAX_PUBLICATION_ATTEMPTS: usize = 8;

impl AgentWorker {
    pub(super) async fn startup_progress(
        &self,
        agent: Agent,
        next_step: AgentWorkStep,
        now: SystemTime,
        evidence: RequestEvidence,
        identity: Option<(String, u64)>,
        sandbox: &Sandbox,
    ) -> Result<(), SandboxStoreError> {
        self.retry_startup_publication(agent, sandbox, now, |mut current, at| {
            let evidence = evidence.clone();
            let identity = identity.clone();
            async move {
                if let Some((conversation_id, generation)) = identity {
                    current.conversation.conversation_id = Some(conversation_id);
                    current.conversation.runtime_generation = Some(generation);
                }
                self.progress(
                    current,
                    next_step,
                    at,
                    Some(evidence),
                    Vec::new(),
                    None,
                    None,
                )
                .await
            }
        })
        .await
    }

    // Only durable publication belongs in this closure, never runtime side effects.
    pub(in crate::sandbox) async fn retry_startup_publication<F, Fut, T>(
        &self,
        mut agent: Agent,
        sandbox: &Sandbox,
        now: SystemTime,
        mut publish: F,
    ) -> Result<T, SandboxStoreError>
    where
        F: FnMut(Agent, SystemTime) -> Fut,
        Fut: Future<Output = Result<T, SandboxStoreError>>,
    {
        let started = tokio::time::Instant::now();
        for attempt in 0..MAX_PUBLICATION_ATTEMPTS {
            let at = now + started.elapsed();
            if !holds_startup_authority(&agent, &self.config.holder, at) {
                return Err(SandboxStoreError::Conflict);
            }
            // Do not embed the full commit future in the retry and open frames.
            // The same attempt stays inside its existing authority and task scope.
            match Box::pin(publish(agent.clone(), at)).await {
                Err(SandboxStoreError::Conflict) if attempt + 1 < MAX_PUBLICATION_ATTEMPTS => {
                    let current = self
                        .dependencies
                        .store
                        .get_agent(&agent.id)
                        .await?
                        .ok_or(SandboxStoreError::NotFound)?;
                    // An aborted ownership-fence write can leave the Agent
                    // completely unchanged. Retry only this durable publication;
                    // the closure must never repeat a native side effect.
                    if current != agent && !only_startup_metadata_changed(&agent, &current) {
                        return Err(SandboxStoreError::Conflict);
                    }
                    let parent = self
                        .dependencies
                        .store
                        .get_sandbox(&agent.sandbox_id)
                        .await?
                        .ok_or(SandboxStoreError::NotFound)?;
                    if parent.id != sandbox.id
                        || parent.owner != agent.owner
                        || parent.status != SandboxStatus::Active
                        || !parent.credential.delivered
                        || parent.lifecycle.checkpoint_capture.is_some()
                        || parent.runtime.is_none()
                        || parent.runtime != sandbox.runtime
                    {
                        return Err(SandboxStoreError::Conflict);
                    }
                    agent = current;
                }
                result => return result,
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

pub(super) fn holds_startup_authority(agent: &Agent, holder: &str, now: SystemTime) -> bool {
    agent.status == AgentStatus::Starting
        && agent.requested_stop_reason.is_none()
        && agent.deleted_at.is_none()
        && matches!(
            agent.work.step,
            AgentWorkStep::AddWorktree
                | AgentWorkStep::MaterializeHome
                | AgentWorkStep::OpenConversation
                | AgentWorkStep::PublishReady
        )
        && agent.work.lease.as_ref().is_some_and(|lease| {
            lease.holder == holder
                && lease.fence > 0
                && seconds_from_rfc3339(&lease.expires_at).is_some_and(|seconds| {
                    now < SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
                })
        })
}

pub(super) fn only_startup_metadata_changed(previous: &Agent, current: &Agent) -> bool {
    if current.revision <= previous.revision
        || current.conversation.last_seq < previous.conversation.last_seq
    {
        return false;
    }
    if previous.conversation.first_message != current.conversation.first_message {
        let Some(first) = current.conversation.first_message.as_ref() else {
            return false;
        };
        if previous.conversation.first_message.is_some()
            || FirstMessageRecord::new(&previous.creation_key, &first.text, &first.queued_at)
                .as_ref()
                != Ok(first)
        {
            return false;
        }
    }
    let mut normalized = current.clone();
    normalized.revision = previous.revision;
    normalized.updated_at = previous.updated_at.clone();
    normalized.title = previous.title.clone();
    normalized.conversation.last_seq = previous.conversation.last_seq;
    normalized.conversation.first_message = previous.conversation.first_message.clone();
    normalized == *previous
}

impl AgentWorker {
    pub(super) async fn publish_ready_once(
        &self,
        mut agent: Agent,
        context: &AgentContext,
        target: &LiveAgentTarget,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let expected = agent.revision;
        let first_ready = agent.ready_at.is_none();
        let publication_started = tokio::time::Instant::now();
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
        let conversation_id = target.conversation_id.clone();
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
        let mut worktree = context.worktree.clone();
        let expected_worktree_revision = worktree.revision;
        worktree
            .transition(WorktreeStatus::Ready)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        self.dependencies
            .store
            .commit_agent_connection_work(
                AgentWorkMutation {
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
                },
                context.publication_parents(),
            )
            .await?;
        if first_ready {
            let ready_at = SystemTime::now();
            let publish_ready_ms = publication_started.elapsed().as_millis() as u64;
            let startup_phases = self.take_startup_timing(&agent.id);
            let runtime_path = super::create_completion::runtime_path(&agent, &context.sandbox);
            self.schedule_completion_event(
                agent,
                "ordinary",
                runtime_path,
                super::create_completion::CompletionTiming {
                    admission_ms: None,
                    admitted_desktop: None,
                    admitted_machine_included: None,
                    ready_at,
                    publish_ready_ms,
                    startup_phases,
                },
            );
        }
        Ok(())
    }
}
