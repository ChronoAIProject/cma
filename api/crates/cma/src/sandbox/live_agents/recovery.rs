use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::sandbox::agent_worker::{CapacityRecoveryReconnect, ReconnectAction};
use layer2_core::sandbox::store::{
    AgentConnectionRecoveryCause, AgentConnectionRecoveryOutcome, AgentConnectionRecoveryRequest,
    AgentLifecycleStore, SandboxStoreError,
};
use layer2_core::{now_rfc3339, AgentId};

use super::{LiveAgentRegistry, RegistryEntry};
use layer2_core::sandbox::{AgentRuntimeError, AgentRuntimeTarget};

const RECOVERY_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveAgentAttachmentIdentity {
    pub(crate) agent_id: AgentId,
    pub(crate) conversation_id: String,
    pub(crate) runtime_generation: u64,
    pub(crate) authority_fence: u64,
    pub(crate) nonce: u64,
}

#[async_trait::async_trait]
pub(crate) trait LiveAgentConnectionLossHandler: Send + Sync {
    async fn connection_lost(&self, attachment: LiveAgentAttachmentIdentity);

    async fn publication_lost(&self, attachment: LiveAgentAttachmentIdentity) {
        self.connection_lost(attachment).await;
    }
}

pub(crate) struct DurableAgentConnectionLossHandler {
    store: Arc<dyn AgentLifecycleStore>,
    capacity_restart: Arc<OnceLock<Arc<dyn CapacityRecoveryReconnect>>>,
}

impl DurableAgentConnectionLossHandler {
    #[cfg(test)]
    pub(crate) fn new(store: Arc<dyn AgentLifecycleStore>) -> Arc<Self> {
        Self::with_capacity_recovery(store, Arc::new(OnceLock::new()))
    }

    pub(crate) fn with_capacity_recovery(
        store: Arc<dyn AgentLifecycleStore>,
        capacity_restart: Arc<OnceLock<Arc<dyn CapacityRecoveryReconnect>>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            capacity_restart,
        })
    }
}

#[async_trait::async_trait]
impl LiveAgentConnectionLossHandler for DurableAgentConnectionLossHandler {
    async fn connection_lost(&self, attachment: LiveAgentAttachmentIdentity) {
        self.recover(attachment, false).await;
    }

    async fn publication_lost(&self, attachment: LiveAgentAttachmentIdentity) {
        self.recover(attachment, true).await;
    }
}

impl DurableAgentConnectionLossHandler {
    async fn recover(&self, attachment: LiveAgentAttachmentIdentity, publication_only: bool) {
        loop {
            let current = match self.store.get_agent(&attachment.agent_id).await {
                Ok(Some(agent)) => agent,
                Ok(None) => return,
                Err(SandboxStoreError::Unavailable(detail)) => {
                    tracing::warn!(
                        agent_id = %attachment.agent_id,
                        %detail,
                        "agent connection recovery read is unavailable"
                    );
                    tokio::time::sleep(RECOVERY_RETRY_DELAY).await;
                    continue;
                }
                Err(error) => {
                    tracing::error!(
                        agent_id = %attachment.agent_id,
                        %error,
                        "agent connection recovery read failed"
                    );
                    return;
                }
            };
            if current.conversation.conversation_id.as_deref() != Some(&attachment.conversation_id)
                || current.conversation.runtime_generation != Some(attachment.runtime_generation)
                || current.work.next_fence <= attachment.authority_fence
                // A newer lifecycle claim owns recovery once publication has
                // completed. Never let its predecessor reschedule that route.
                || (publication_only
                    && attachment.authority_fence.checked_add(1)
                        != Some(current.work.next_fence))
            {
                return;
            }
            if let Some(recovery) = self.capacity_restart.get() {
                match recovery.route(&attachment.agent_id).await {
                    Ok(ReconnectAction::Ordinary) => {}
                    // Failed/unknown attachment outcomes remain durable and
                    // are retried by the restart sweep. Never reopen or replay.
                    Ok(ReconnectAction::Recovered | ReconnectAction::Deferred) | Err(_) => return,
                }
            }
            let request = AgentConnectionRecoveryRequest {
                agent_id: attachment.agent_id.clone(),
                conversation_id: attachment.conversation_id.clone(),
                runtime_generation: attachment.runtime_generation,
                expected_revision: current.revision,
                expected_work_step: current.work.step,
                expected_authority_fence: Some(attachment.authority_fence),
                requested_at: now_rfc3339(),
                cause: AgentConnectionRecoveryCause::UpdateStreamLost,
            };
            match self.store.schedule_agent_connection_recovery(request).await {
                Ok(
                    AgentConnectionRecoveryOutcome::Scheduled
                    | AgentConnectionRecoveryOutcome::AlreadyScheduled
                    | AgentConnectionRecoveryOutcome::NotEligible,
                ) => return,
                Ok(AgentConnectionRecoveryOutcome::Superseded)
                | Err(SandboxStoreError::Conflict) => tokio::task::yield_now().await,
                Err(SandboxStoreError::Unavailable(detail)) => {
                    tracing::warn!(
                        agent_id = %attachment.agent_id,
                        %detail,
                        "agent connection recovery scheduling is unavailable"
                    );
                    tokio::time::sleep(RECOVERY_RETRY_DELAY).await;
                }
                Err(error) => {
                    tracing::error!(
                        agent_id = %attachment.agent_id,
                        %error,
                        "agent connection recovery scheduling failed"
                    );
                    return;
                }
            }
        }
    }
}

impl LiveAgentRegistry {
    pub(super) async fn reconnect_target(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<(), AgentRuntimeError> {
        let identity = {
            let state = self.lock();
            let entry = state
                .entries
                .get(&target.agent_id)
                .ok_or(AgentRuntimeError::NotFound)?;
            if entry.runtime_generation != target.runtime_generation
                || entry.conversation_id != target.conversation_id
            {
                return Err(AgentRuntimeError::StaleGeneration);
            }
            LiveAgentAttachmentIdentity {
                agent_id: target.agent_id.clone(),
                conversation_id: target.conversation_id.clone(),
                runtime_generation: target.runtime_generation,
                authority_fence: entry.authority_fence,
                nonce: entry.nonce,
            }
        };
        self.connection_ended(identity).await;
        Ok(())
    }

    pub(crate) fn install_connection_loss_handler(
        &self,
        handler: Arc<dyn LiveAgentConnectionLossHandler>,
    ) -> bool {
        self.loss_handler.set(handler).is_ok()
    }

    pub(crate) async fn connection_ended(&self, attachment: LiveAgentAttachmentIdentity) {
        let handler = {
            let mut state = self.lock();
            let exact = state
                .entries
                .get(&attachment.agent_id)
                .is_some_and(|entry| exact_attachment(entry, &attachment));
            if !exact {
                let opening = state.staged.get(&attachment.agent_id).is_some_and(|entry| {
                    same_attachment(entry, &attachment)
                        && !matches!(
                            entry.input,
                            super::AttachmentInput::Connected {
                                activation: Some(_),
                                ..
                            }
                        )
                });
                if opening {
                    super::fence_authority(
                        &mut state,
                        &attachment.agent_id,
                        attachment.runtime_generation,
                        attachment.authority_fence,
                    );
                    state.staged.remove(&attachment.agent_id);
                }
                return;
            }
            let owns_current_authority =
                state
                    .watermarks
                    .get(&attachment.agent_id)
                    .is_some_and(|watermark| {
                        !watermark.fenced
                            && watermark.runtime_generation == attachment.runtime_generation
                            && watermark.authority_fence == attachment.authority_fence
                    });
            if owns_current_authority {
                state
                    .watermarks
                    .get_mut(&attachment.agent_id)
                    .expect("current attachment has an authority watermark")
                    .fenced = true;
            }
            state.entries.remove(&attachment.agent_id);
            owns_current_authority
                .then(|| self.loss_handler.get().cloned())
                .flatten()
        };
        if let Some(handler) = handler {
            handler.connection_lost(attachment).await;
        }
    }
}

impl super::LiveAgentAttachment {
    pub(crate) fn identity(&self) -> LiveAgentAttachmentIdentity {
        LiveAgentAttachmentIdentity {
            agent_id: self.agent_id.clone(),
            conversation_id: self.conversation_id.clone(),
            runtime_generation: self.runtime_generation,
            authority_fence: self.authority_fence,
            nonce: self.nonce,
        }
    }
}

fn exact_attachment(entry: &RegistryEntry, attachment: &LiveAgentAttachmentIdentity) -> bool {
    entry.published && same_attachment(entry, attachment)
}

fn same_attachment(entry: &RegistryEntry, attachment: &LiveAgentAttachmentIdentity) -> bool {
    entry.conversation_id == attachment.conversation_id
        && entry.runtime_generation == attachment.runtime_generation
        && entry.authority_fence == attachment.authority_fence
        && entry.nonce == attachment.nonce
}
