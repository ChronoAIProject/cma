use super::*;
use layer2_core::{agent_ownership::OwnershipError, sandbox::AgentWorkStep};

impl AgentManager {
    // Control-only ownership of retired Agents must not consume capacity forever.
    // Round-robin bounded inspection also covers owners acquired by maintenance.
    pub(super) async fn retire_inactive(&self) -> Result<(), OwnershipError> {
        let owned = self.coordinator.owned().await;
        if owned.is_empty() {
            return Ok(());
        }
        let start = self.retired_cursor.fetch_add(4, Ordering::Relaxed) % owned.len();
        for index in 0..owned.len().min(4) {
            let authority = &owned[(start + index) % owned.len()];
            // Receiver cancellation has its own bounded cleanup and retry path.
            if authority.receiver_cancelled() {
                continue;
            }
            let observed = tokio::time::timeout(
                Duration::from_secs(2),
                self.store.get_agent(&authority.token.agent_id),
            )
            .await;
            let retired = match observed {
                Ok(Ok(agent)) => agent.is_none_or(|agent| {
                    !agent.status.is_live() && agent.work.step == AgentWorkStep::None
                }),
                _ => continue,
            };
            let eligible = {
                let mut since = self.retired_since.lock().unwrap();
                if retired {
                    since
                        .entry(authority.token.agent_id.clone())
                        .or_insert_with(Instant::now)
                        .elapsed()
                        >= Duration::from_secs(60)
                } else {
                    since.remove(&authority.token.agent_id);
                    false
                }
            };
            if eligible {
                self.registry.detach_owner(&authority.token);
                match self.handoff_receiver(authority).await {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(error) => {
                        self.contained
                            .contain("inactive_retirement", authority, error)?;
                        continue;
                    }
                }
                self.retired_since
                    .lock()
                    .unwrap()
                    .remove(&authority.token.agent_id);
            }
        }
        Ok(())
    }
}
