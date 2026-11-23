use std::sync::{Arc, Weak};
use std::time::Duration;

use layer2_core::budget::BudgetStop;
use layer2_core::sandbox::AgentUpdateRecorder;
use layer3_adapter::{
    AdapterError, ConversationInput, ConversationInputGuard, ConversationInputSender,
};

use super::{LiveAgentAttachmentIdentity, LiveAgentRegistry};

pub(super) struct BudgetMonitor(tokio::task::AbortHandle);

impl Drop for BudgetMonitor {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) fn monitor(
    registry: Weak<LiveAgentRegistry>,
    attachment: LiveAgentAttachmentIdentity,
    recorder: Arc<dyn AgentUpdateRecorder>,
) -> BudgetMonitor {
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let stop = match recorder
                .budget_stop(&attachment.agent_id, attachment.runtime_generation)
                .await
            {
                Ok(Some(stop))
                    if stop.agent_id == attachment.agent_id
                        && stop.runtime_generation == attachment.runtime_generation =>
                {
                    stop
                }
                Ok(Some(_)) => continue,
                Ok(None) => continue,
                Err(_) => {
                    tracing::warn!("model budget stop observation is unavailable");
                    continue;
                }
            };
            let Some(input) = current_input(&registry, &attachment) else {
                // Recovery may already record output while its command route
                // awaits publication. The consumer owns and cancels this monitor.
                continue;
            };
            let guard = Arc::new(BudgetInterruptGuard {
                registry: registry.clone(),
                attachment: attachment.clone(),
                recorder: recorder.clone(),
                stop,
            });
            // Confirmation means cancellation was sent, not that the turn ended.
            // Only the runtime's terminal evidence allows durable settlement.
            if !matches!(
                input.send_guarded(ConversationInput::Cancel, guard).await,
                layer3_adapter::ConversationSendOutcome::WriteConfirmed
            ) {
                tracing::warn!("model budget interruption awaits runtime confirmation");
            }
        }
    });
    BudgetMonitor(task.abort_handle())
}

fn current_input(
    registry: &Weak<LiveAgentRegistry>,
    attachment: &LiveAgentAttachmentIdentity,
) -> Option<ConversationInputSender> {
    let registry = registry.upgrade()?;
    let state = registry.lock();
    state
        .entries
        .get(&attachment.agent_id)
        .filter(|entry| {
            entry.published
                && entry.runtime_generation == attachment.runtime_generation
                && entry.authority_fence == attachment.authority_fence
                && entry.nonce == attachment.nonce
                && entry.conversation_id == attachment.conversation_id
        })
        .and_then(|entry| entry.input.sender().cloned())
}

struct BudgetInterruptGuard {
    registry: Weak<LiveAgentRegistry>,
    attachment: LiveAgentAttachmentIdentity,
    recorder: Arc<dyn AgentUpdateRecorder>,
    stop: BudgetStop,
}

#[async_trait::async_trait]
impl ConversationInputGuard for BudgetInterruptGuard {
    async fn check(&self) -> layer3_adapter::Result<()> {
        if self.stop.agent_id != self.attachment.agent_id
            || self.stop.runtime_generation != self.attachment.runtime_generation
            || current_input(&self.registry, &self.attachment).is_none()
        {
            return Err(AdapterError::Rejected(
                "budget stop attachment changed".into(),
            ));
        }
        let current = self
            .recorder
            .budget_stop(
                &self.attachment.agent_id,
                self.attachment.runtime_generation,
            )
            .await
            .map_err(|_| {
                AdapterError::InputAuthorityUnavailable("budget stop is unavailable".into())
            })?;
        if current.as_ref() != Some(&self.stop) {
            return Err(AdapterError::Rejected("budget stop turn changed".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "budget_tests.rs"]
mod tests;
