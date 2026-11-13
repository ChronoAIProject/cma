use std::future::Future;
use std::time::{Duration, SystemTime};

use layer2_core::profiles::ModelAccess;
use layer2_core::sandbox::store::AgentWorkMutation;
use layer2_core::sandbox::{Agent, AgentLogKind, Sandbox};
use layer3_adapter::{AgentOpenRequest, ConversationChannel, ConversationUpdate};
use tokio::sync::mpsc;

use super::support::{
    adapter_failure, bump_log_sequence, claimed_fence, AgentContext, WorkerFailure,
};
use super::AgentWorker;

const UPDATE_BUFFER: usize = 256;

impl AgentWorker {
    pub(super) async fn open_starting(
        &self,
        agent: &mut Agent,
        context: &AgentContext,
        authority_fence: u64,
        request: &AgentOpenRequest,
        now: SystemTime,
    ) -> Result<(ConversationChannel, mpsc::Receiver<ConversationUpdate>), WorkerFailure> {
        let (updates, receiver) = mpsc::channel(UPDATE_BUFFER);
        let budget = if context.sandbox.profile.content.model_access == ModelAccess::OwnCredentials
        {
            self.config.sign_in_timeout
        } else {
            self.config.open_timeout
        };
        let warm_conversation = agent.work.warm_conversation_id.clone();
        let opening = async {
            if let Some(conversation_id) = warm_conversation.as_deref() {
                self.dependencies
                    .adapter
                    .reconnect_agent(
                        &context.runtime,
                        authority_fence,
                        request,
                        conversation_id,
                        updates,
                    )
                    .await
            } else {
                self.dependencies
                    .adapter
                    .open_agent(&context.runtime, authority_fence, request, updates)
                    .await
            }
        };
        self.await_startup_open(agent, &context.sandbox, receiver, opening, budget, now)
            .await
    }

    pub(in crate::sandbox) async fn await_startup_open<F>(
        &self,
        agent: &mut Agent,
        sandbox: &Sandbox,
        mut receiver: mpsc::Receiver<ConversationUpdate>,
        opening: F,
        budget: Duration,
        now: SystemTime,
    ) -> Result<(ConversationChannel, mpsc::Receiver<ConversationUpdate>), WorkerFailure>
    where
        F: Future<Output = layer3_adapter::Result<ConversationChannel>>,
    {
        let started = tokio::time::Instant::now();
        let expires = agent
            .work
            .lease
            .as_ref()
            .and_then(|lease| layer2_core::seconds_from_rfc3339(&lease.expires_at))
            .and_then(|seconds| {
                (SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
                    .duration_since(now)
                    .ok()
            })
            .unwrap_or_default()
            .saturating_sub(Duration::from_secs(1));
        let deadline = tokio::time::sleep(budget.min(expires));
        tokio::pin!(deadline);
        tokio::pin!(opening);
        let mut buffered = Vec::new();
        let mut received = 0;
        let mut updates_open = true;
        loop {
            tokio::select! {
                biased;
                _ = &mut deadline => return Err(WorkerFailure::retryable(
                    "agent_conversation_open_timeout", "conversation did not become ready within its admitted startup deadline",
                )),
                update = receiver.recv(), if updates_open => {
                    let Some(update) = update else {
                        updates_open = false;
                        continue;
                    };
                    // Reliable frames are bounded at the retained runtime journal and
                    // adapter ingress. Replaying more than 256 is valid after a detach.
                    if !matches!(&update, ConversationUpdate::RuntimeOutput { .. }) {
                        received += 1;
                    }
                    if received > UPDATE_BUFFER {
                        return Err(WorkerFailure::terminal("agent_startup_update_limit", "runtime emitted too many startup updates"));
                    }
                    match update {
                        ConversationUpdate::SignInRequired { url, code, message } => {
                            self.record_startup_notice(agent, sandbox, AgentLogKind::SignInRequired { url, code, message }, now + started.elapsed()).await?;
                        }
                        ConversationUpdate::RetainedEffect(ref effect)
                            if matches!(effect.effect(), ConversationUpdate::SignInRequired { .. }) => {
                            let ConversationUpdate::SignInRequired { url, code, message } = effect.effect() else { unreachable!() };
                            self.record_startup_notice(agent, sandbox, AgentLogKind::SignInRequired { url: url.clone(), code: code.clone(), message: message.clone() }, now + started.elapsed()).await?;
                        }
                        other => buffered.push(other),
                    }
                }
                opened = &mut opening => {
                    let channel = opened.map_err(adapter_failure)?;
                    return Ok((channel, prepend_updates(buffered, receiver)));
                }
            }
        }
    }

    async fn record_startup_notice(
        &self,
        current: &mut Agent,
        sandbox: &Sandbox,
        notice: AgentLogKind,
        now: SystemTime,
    ) -> Result<(), WorkerFailure> {
        *current = self
            .retry_startup_publication(current.clone(), sandbox, now, |agent, at| {
                self.record_startup_notice_once(agent, notice.clone(), at)
            })
            .await
            .map_err(publication_failure)?;
        Ok(())
    }

    async fn record_startup_notice_once(
        &self,
        mut agent: Agent,
        notice: AgentLogKind,
        now: SystemTime,
    ) -> Result<Agent, layer2_core::sandbox::store::SandboxStoreError> {
        let stamp = layer2_core::rfc3339_at(now);
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        agent.revision = agent
            .revision
            .checked_add(1)
            .ok_or(layer2_core::sandbox::store::SandboxStoreError::Conflict)?;
        agent.updated_at = stamp.clone();
        bump_log_sequence(&mut agent, 1)?;
        self.dependencies
            .store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: expected,
                holder: self.config.holder.clone(),
                fence,
                agent: agent.clone(),
                worktree: None,
                appended_at: stamp,
                sandbox_event: None,
                agent_logs: vec![notice],
            })
            .await?;
        Ok(agent)
    }
}

fn prepend_updates(
    buffered: Vec<ConversationUpdate>,
    mut receiver: mpsc::Receiver<ConversationUpdate>,
) -> mpsc::Receiver<ConversationUpdate> {
    if buffered.is_empty() {
        return receiver;
    }
    let (sender, output) = mpsc::channel(UPDATE_BUFFER);
    tokio::spawn(async move {
        for update in buffered {
            if sender.send(update).await.is_err() {
                return;
            }
        }
        loop {
            tokio::select! {
                _ = sender.closed() => return,
                update = receiver.recv() => {
                    let Some(update) = update else { return };
                    if sender.send(update).await.is_err() {
                        return;
                    }
                }
            }
        }
    });
    output
}

fn publication_failure(error: layer2_core::sandbox::store::SandboxStoreError) -> WorkerFailure {
    WorkerFailure::retryable("agent_startup_notice_unavailable", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn startup_relay_preserves_order_and_releases_upstream_when_detached() {
        let (upstream, receiver) = mpsc::channel(4);
        let mut output = prepend_updates(vec![ConversationUpdate::Lost { frames: 1 }], receiver);
        upstream
            .send(ConversationUpdate::Lost { frames: 2 })
            .await
            .unwrap();
        assert!(matches!(
            output.recv().await,
            Some(ConversationUpdate::Lost { frames: 1 })
        ));
        assert!(matches!(
            output.recv().await,
            Some(ConversationUpdate::Lost { frames: 2 })
        ));
        drop(output);
        tokio::time::timeout(Duration::from_secs(1), upstream.closed())
            .await
            .expect("detaching the consumer releases an otherwise idle upstream receiver");
    }
}
