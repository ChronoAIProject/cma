//! One configured, owned connection with lease renewal. No automatic activation.
use super::{
    status::{FailureCode, Stage},
    DurableJournal, EndReason, GatewayClient, GatewayEnd, GatewayError,
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::sync::watch;

/// Cleanup makes a few individually bounded store calls. An unconfirmed write
/// leaves the persisted lease in charge of when another worker can acquire it.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);

impl DurableJournal {
    /// The caller supplies an already owned source. Shutdown or authority loss
    /// closes the socket; this method never falls back to another Identify.
    /// A supervisor must obey retry_after and reacquire before reconnecting.
    pub async fn run_connection(
        self: Arc<Self>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<GatewayEnd, GatewayError> {
        self.status.step(Stage::Identify);
        let gateway = GatewayClient::new(
            self.authority.token.clone(),
            self.authority.bot_id.clone(),
            self.authority.intents,
        )
        .inspect_err(|error| self.status.failure((*error).into()))?
        .with_status(self.status.clone());
        let resume = self.checkpoint().await?;
        let (stop, receiver) = watch::channel(*shutdown.borrow());
        let connection = gateway.connect(resume, self.clone(), receiver);
        let renew_every = if self.store.replica.is_some() {
            crate::replicas::config::RENEW
        } else {
            Duration::from_secs(10)
        };
        self.supervise_connection(connection, stop, shutdown, renew_every)
            .await
    }

    pub(super) async fn supervise_connection(
        &self,
        connection: impl Future<Output = Result<GatewayEnd, GatewayError>>,
        stop: watch::Sender<bool>,
        shutdown: watch::Receiver<bool>,
        renew_every: Duration,
    ) -> Result<GatewayEnd, GatewayError> {
        let result = self
            .renew_while_connected(connection, stop, shutdown, renew_every)
            .await;
        match &result {
            Ok(end) if end.reason != EndReason::Stopped => self.status.failure(end.reason.into()),
            Err(error) => self.status.failure((*error).into()),
            _ => {}
        }
        // A missing close acknowledgement retains the lease until expiry.
        // Failed connection setup has never sent Identify and owns no socket.
        let closed = result.as_ref().map_or(true, |end| end.transport_closed);
        let released = match &result {
            Ok(end) if end.checkpoint.is_none() => {
                // The checkpoint and lease must change together on a confirmed close.
                let discarded = bounded(self.discard_checkpoint(closed)).await;
                closed && discarded
            }
            _ => closed && bounded(self.release()).await,
        };
        if !closed {
            tracing::warn!("native Discord close unconfirmed; retaining source lease until expiry");
        }
        if !released {
            self.status.cleanup_failed();
        }
        self.status.lease_released(released);
        result
    }

    /// The renewal future is owned by this call and dropped when it returns.
    /// A renewal suspended inside `renew` holds the serial guard, or a queued
    /// claim on it, that every cleanup step needs: it must never outlive the
    /// connection it renews.
    async fn renew_while_connected(
        &self,
        connection: impl Future<Output = Result<GatewayEnd, GatewayError>>,
        stop: watch::Sender<bool>,
        mut shutdown: watch::Receiver<bool>,
        renew_every: Duration,
    ) -> Result<GatewayEnd, GatewayError> {
        tokio::pin!(connection);
        let renewal = async {
            let replica = self.store.replica.is_some();
            loop {
                tokio::time::sleep(renew_every).await;
                match tokio::time::timeout(
                    if replica {
                        crate::replicas::config::STORE_TIMEOUT
                    } else {
                        Duration::from_secs(15)
                    },
                    self.renew(),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    result => {
                        self.status.step(Stage::Renew);
                        self.status.failure(if result.is_err() {
                            FailureCode::RenewTimeout
                        } else {
                            FailureCode::RenewFailed
                        });
                        break;
                    }
                }
            }
        };
        tokio::pin!(renewal);
        loop {
            tokio::select! {
                result = &mut connection => break result,
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        let _ = stop.send(true);
                        break connection.await;
                    }
                }
                _ = &mut renewal => {
                    let _ = stop.send(true);
                    break connection.await.map(|mut result| {
                        result.reason = EndReason::AuthorityRevoked;
                        result
                    });
                }
            }
        }
    }
}

async fn bounded(step: impl Future<Output = Result<(), GatewayError>>) -> bool {
    matches!(
        tokio::time::timeout(CLEANUP_TIMEOUT, step).await,
        Ok(Ok(()))
    )
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
