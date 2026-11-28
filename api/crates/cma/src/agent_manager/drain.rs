//! Successful local joins and receiver revocation precede durable release.
use super::*;
use futures_util::{stream, StreamExt};
use layer2_core::agent_ownership::{Authority, OwnershipError};
use std::future::Future;

#[derive(Debug)]
pub(crate) struct HandoffFailure {
    phase: &'static str,
    error: OwnershipError,
}

pub(crate) async fn handoff<F: Future<Output = Result<(), OwnershipError>>>(
    coordinator: &OwnershipCoordinator,
    authority: &Arc<Authority>,
    until: Instant,
    mut revoke: impl FnMut() -> F,
) -> Result<(), HandoffFailure> {
    let mut receivers_settled = false;
    let result = coordinator
        .handoff_during_drain(
            authority,
            async {
                // Revoke carries the exact owner token. A retry after an
                // unconfirmed result revisits only unsettled receivers.
                match revoke().await {
                    Err(OwnershipError::Unavailable(_)) => revoke().await?,
                    result => result?,
                }
                receivers_settled = true;
                Ok(())
            },
            until,
        )
        .await;
    if result.is_ok() && receivers_settled {
        let correlation = authority.receiver_correlation();
        tracing::info!(parent: None, stage = "durable_release", outcome = "released",
            agent_sha256 = %correlation.agent_sha256, instance_sha256 = %correlation.instance_sha256,
            epoch = authority.token.epoch, "Agent Manager drain handoff confirmed");
    }
    result.map_err(|error| {
        let phase = if receivers_settled {
            "durable_release"
        } else {
            "receiver_revocation"
        };
        let failure = HandoffFailure { phase, error };
        authority.log_receiver_handoff_failure(
            failure.phase,
            &failure.error,
            "Agent Manager drain handoff failed",
        );
        failure
    })
}

impl AgentManager {
    pub(crate) fn register_retirement_capture(
        &self,
        capture: crate::agent_lifecycle::LifecycleDrain,
    ) -> Result<(), String> {
        self.retirement_capture
            .set(capture)
            .map_err(|_| "Agent Manager retirement capture already registered".into())
    }

    pub(crate) fn stop_admission(&self) {
        self.serving.store(false, Ordering::Release);
        self.coordinator.stop_admission();
        if let Some(capture) = self.retirement_capture.get() {
            capture.close();
        }
        if let Some(lifecycle) = self.lifecycle.get() {
            lifecycle.close();
        }
    }

    pub async fn drain(&self) -> Result<(), String> {
        if self.coordinator.contains_failures() {
            return self
                .drain_contained(
                    DrainCause::PreStop,
                    Instant::now() + Duration::from_secs(23),
                )
                .await;
        }
        self.stop_admission();
        // Include serialization, membership IO, task joins and all receiver IO
        // in one budget below the signed preStop client's 25-second timeout.
        let until = Instant::now() + Duration::from_secs(23);
        let result = tokio::time::timeout_at(until, self.drain_inner(until)).await;
        match result {
            Ok(Ok(())) => Ok(()),
            result => {
                self.coordinator.fail().await;
                tracing::warn!("Agent Manager drain retained unconfirmed leases for expiry");
                Err(match result {
                    Ok(Err(reason)) => reason,
                    _ => "Agent Manager drain exceeded its deadline".into(),
                })
            }
        }
    }

    pub(super) async fn drain_retirement_capture(&self, until: Instant) -> Result<(), String> {
        if let Some(capture) = self.retirement_capture.get() {
            capture
                .drain(Duration::from_secs(5).min(until.saturating_duration_since(Instant::now())))
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    async fn drain_inner(&self, until: Instant) -> Result<(), String> {
        let mut drained = self.drain.lock().await;
        if *drained {
            return Ok(());
        }
        let custody = self.coordinator.prepare_drain(until).await;
        let owned = self.coordinator.owned().await;
        for authority in &owned {
            authority.revoke();
        }
        self.lifecycle
            .get()
            .ok_or("Agent Manager lifecycle drain is missing")?
            .drain(Duration::from_secs(5))
            .await
            .map_err(|error| error.to_string())?;
        self.drain_retirement_capture(until).await?;
        let results = stream::iter(owned.into_iter().map(|authority| async move {
            self.registry.detach_owner(&authority.token);
            handoff(&self.coordinator, &authority, until, || {
                authority.revoke_transports_after_local_drain()
            })
            .await
        }))
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
        if results.iter().any(Result::is_err) {
            return Err("Agent Manager receiver drain was not confirmed".into());
        }
        custody.map_err(|_| "Agent Manager lease operation drain was not confirmed")?;
        *drained = true;
        Ok(())
    }
}

#[cfg(test)]
#[path = "drain_tests.rs"]
mod tests;
