use super::*;
use futures_util::{stream, StreamExt};
use layer2_core::agent_ownership::Settlement;

impl AgentManager {
    pub(crate) async fn drain_contained(
        &self,
        cause: DrainCause,
        process_until: Instant,
    ) -> Result<(), String> {
        let episode = self.drain_episode.get_or_init(|| {
            let episode = drain_episode::DrainEpisode::new(cause, process_until);
            self.stop_admission();
            episode
        });
        let result = episode
            .run(self.finish_contained_drain(episode.until))
            .await;
        if result.is_err() {
            self.coordinator.fail().await;
            // Aborting is not proof of durable rollback. Retain unresolved
            // records and observe termination within this same episode only.
            let _ = self.coordinator.abort_operations_until(episode.until).await;
        }
        result
    }

    async fn finish_contained_drain(&self, until: Instant) -> Result<(), String> {
        let supervisor = self.join_supervisor(until).await;
        let custody = self.coordinator.prepare_drain(until).await;
        self.lifecycle
            .get()
            .ok_or("Agent Manager lifecycle drain is missing")?
            .drain(Duration::from_secs(5).min(until.saturating_duration_since(Instant::now())))
            .await
            .map_err(|_| "Agent Manager local lifecycle drain was not confirmed")?;
        self.drain_retirement_capture(until).await?;
        // Publication is closed and lifecycle work has terminated. Include
        // late known acquisition results and every noncurrent responsibility.
        let authorities = self.coordinator.owned().await;
        let results = stream::iter(authorities)
            .map(|authority| async move {
                self.registry.detach_owner(&authority.token);
                let result = self
                    .coordinator
                    .drain_authority_until(&authority, until)
                    .await;
                tracing::info!(parent: None,
                    stage = "receiver_drain",
                    outcome = match &result {
                        Ok(Settlement::Released) => "released",
                        Ok(Settlement::Superseded { .. }) => "superseded",
                        Err(_) => "incomplete",
                    },
                    epoch = authority.token.epoch,
                    "Agent Manager receiver drain result"
                );
                result
            })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        supervisor?;
        custody.map_err(|_| "Agent Manager lease operation drain was not confirmed")?;
        if results.iter().any(Result::is_err) {
            return Err("Agent Manager receiver drain was not confirmed".into());
        }
        Ok(())
    }

    async fn join_supervisor(&self, until: Instant) -> Result<(), String> {
        let mut retained = tokio::time::timeout_at(until, self.supervisor.lock())
            .await
            .map_err(|_| "Agent Manager supervisor join was not confirmed")?;
        if let Some(outcome) = self.supervisor_outcome.get() {
            return outcome.clone();
        }
        let Some(supervisor) = retained.as_mut() else {
            return Ok(());
        };
        let cooperative_until = until.min(Instant::now() + Duration::from_secs(2));
        let result = match tokio::time::timeout_at(cooperative_until, &mut *supervisor).await {
            Ok(result) => result,
            Err(_) => {
                supervisor.abort();
                tokio::time::timeout_at(until, &mut *supervisor)
                    .await
                    .map_err(|_| "Agent Manager supervisor termination was not confirmed")?
            }
        };
        let outcome = match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(())) => Err("Agent Manager supervisor reported an ownership failure".into()),
            Err(error) if error.is_cancelled() => Ok(()),
            Err(_) => Err("Agent Manager supervisor terminated unexpectedly".into()),
        };
        // A cancelled drain caller may resume receiver cleanup, but cannot
        // forget a supervisor result that was already observed and consumed.
        let outcome = self.supervisor_outcome.get_or_init(|| outcome).clone();
        retained.take();
        outcome
    }
}
