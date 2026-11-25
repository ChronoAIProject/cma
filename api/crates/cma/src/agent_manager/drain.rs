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

pub(crate) async fn handoff(
    coordinator: &OwnershipCoordinator,
    authority: &Arc<Authority>,
    revoke: impl Future<Output = Result<(), OwnershipError>>,
) -> Result<(), HandoffFailure> {
    let mut receivers_settled = false;
    let result = coordinator
        .handoff(authority, async {
            revoke.await?;
            receivers_settled = true;
            Ok(())
        })
        .await;
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
        self.stop_admission();
        // Include serialization, membership IO, task joins and all receiver IO
        // in one budget below the signed preStop client's 25-second timeout.
        let result = tokio::time::timeout(Duration::from_secs(23), self.drain_inner()).await;
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

    async fn drain_inner(&self) -> Result<(), String> {
        let mut drained = self.drain.lock().await;
        if *drained {
            return Ok(());
        }
        let owned = self.coordinator.begin_drain().await;
        self.lifecycle
            .get()
            .ok_or("Agent Manager lifecycle drain is missing")?
            .drain(Duration::from_secs(5))
            .await
            .map_err(|error| error.to_string())?;
        if let Some(capture) = self.retirement_capture.get() {
            capture
                .drain(Duration::from_secs(5))
                .await
                .map_err(|error| error.to_string())?;
        }
        let results = stream::iter(owned.into_iter().map(|authority| async move {
            self.registry.detach_owner(&authority.token);
            handoff(
                &self.coordinator,
                &authority,
                authority.revoke_transports_after_local_drain(),
            )
            .await
        }))
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
        if results.iter().any(Result::is_err) {
            return Err("Agent Manager receiver drain was not confirmed".into());
        }
        *drained = true;
        Ok(())
    }
}

#[cfg(test)]
#[path = "drain_tests.rs"]
mod tests;
