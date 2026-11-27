use super::*;
use layer2_core::agent_ownership::{Authority, BackgroundHandoffError, OwnershipError};

impl AgentManager {
    pub(super) async fn handoff_receiver(
        &self,
        authority: &Arc<Authority>,
    ) -> Result<bool, BackgroundHandoffError> {
        match self
            .coordinator
            .handoff_background_classified(authority)
            .await
        {
            Ok(completed) => Ok(completed),
            Err(error) if authority.receiver_cancelled() => {
                authority.log_receiver_handoff_failure(
                    "cancelled_receiver_cleanup",
                    &error.into_ownership_error(),
                    "Receiver cleanup pending; healthy ownership continues",
                );
                match self.coordinator.forget_cancelled(authority).await {
                    Ok(complete) => Ok(complete),
                    Err(error) => {
                        authority.log_receiver_handoff_failure(
                            "cancelled_owner_observation",
                            &error,
                            "Cancelled ownership retained until durable observation succeeds",
                        );
                        Ok(false)
                    }
                }
            }
            Err(error) => Err(error),
        }
    }

    pub(super) async fn maintain_receivers(
        &self,
        candidates: Vec<Arc<Authority>>,
    ) -> Result<(), OwnershipError> {
        if self.coordinator.contains_failures() {
            return self.service_retirements(candidates).await;
        }
        self.coordinator.poll_acquisitions().await?;
        self.retire_inactive().await?;
        let completed = self
            .contained
            .retry(&self.coordinator, |authority| async move {
                self.handoff_receiver(&authority)
                    .await
                    .map_err(BackgroundHandoffError::into_ownership_error)
            })
            .await?;
        for id in completed {
            self.handoffs.lock().unwrap().remove(&id);
            self.retired_since.lock().unwrap().remove(&id);
        }
        let cancelled = self.coordinator.cancelled_receivers().await;
        for authority in cancelled.into_iter().chain(candidates) {
            if authority.receiver_cancelled() {
                self.registry.detach_owner(&authority.token);
                // A failed receiver remains fenced and is retried independently
                // of healthy owners. Only confirmed revocation releases a lease.
                if self
                    .handoff_receiver(&authority)
                    .await
                    .map_err(BackgroundHandoffError::into_ownership_error)?
                {
                    self.handoffs
                        .lock()
                        .unwrap()
                        .remove(&authority.token.agent_id);
                    self.retired_since
                        .lock()
                        .unwrap()
                        .remove(&authority.token.agent_id);
                }
                continue;
            }
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
            match self.handoff_receiver(&authority).await {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    self.contained
                        .contain("current_handoff", &authority, error)?;
                    continue;
                }
            }
            self.handoffs
                .lock()
                .unwrap()
                .remove(&authority.token.agent_id);
        }
        Ok(())
    }
}
