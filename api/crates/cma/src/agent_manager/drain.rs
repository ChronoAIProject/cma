//! Successful local joins and receiver revocation precede durable release.
use super::*;
use futures_util::{stream, StreamExt};

impl AgentManager {
    pub(crate) fn stop_admission(&self) {
        self.serving.store(false, Ordering::Release);
        self.coordinator.stop_admission();
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
        let results = stream::iter(owned.into_iter().map(|authority| async move {
            self.registry.detach_owner(&authority.token);
            self.coordinator
                .handoff(&authority, authority.revoke_transports())
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
