use super::*;
use layer2_core::agent_ownership::{Authority, OwnershipError};

impl AgentManager {
    pub(super) async fn maintain_receivers(
        &self,
        candidates: Vec<Arc<Authority>>,
    ) -> Result<(), OwnershipError> {
        self.retire_inactive().await?;
        for authority in candidates {
            if !authority.is_current() {
                continue;
            }
            let started = *self
                .handoffs
                .lock()
                .unwrap()
                .entry(authority.token.agent_id.clone())
                .or_insert_with(Instant::now);
            let busy = tokio::time::timeout(
                Duration::from_secs(2),
                self.store.get_agent(&authority.token.agent_id),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .is_none_or(|agent| {
                agent.conversation.active_turn.is_some() || agent.work.lease.is_some()
            });
            if busy && started.elapsed() < Duration::from_secs(60) {
                continue;
            }
            self.registry.detach_owner(&authority.token);
            self.coordinator
                .handoff(&authority, authority.revoke_transports())
                .await?;
            self.handoffs
                .lock()
                .unwrap()
                .remove(&authority.token.agent_id);
        }
        Ok(())
    }
}
