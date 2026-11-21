//! One configured, owned connection with lease renewal. No automatic activation.
use super::{
    status::{FailureCode, Stage},
    DurableJournal, EndReason, GatewayClient, GatewayEnd, GatewayError,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;
impl DurableJournal {
    /// The caller supplies an already owned source. Shutdown or authority loss
    /// closes the socket; this method never falls back to another Identify.
    /// A supervisor must obey retry_after and reacquire before reconnecting.
    pub async fn run_connection(
        self: Arc<Self>,
        mut shutdown: watch::Receiver<bool>,
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
        tokio::pin!(connection);
        let renewal = async {
            let replica = self.store.replica.is_some();
            loop {
                tokio::time::sleep(if replica {
                    crate::replicas::config::RENEW
                } else {
                    Duration::from_secs(10)
                })
                .await;
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
        let result = loop {
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
        };
        match &result {
            Ok(end) if end.reason != EndReason::Stopped => self.status.failure(end.reason.into()),
            Err(error) => self.status.failure((*error).into()),
            _ => {}
        }
        if result.as_ref().is_ok_and(|end| end.checkpoint.is_none())
            && self.discard_checkpoint().await.is_err()
        {
            self.status.cleanup_failed();
        }
        // A missing close acknowledgement retains the lease until expiry.
        // Failed connection setup has never sent Identify and owns no socket.
        let closed = result.as_ref().map_or(true, |end| end.transport_closed);
        let released = closed && self.release().await.is_ok();
        if !closed {
            tracing::warn!("native Discord close unconfirmed; retaining source lease until expiry");
        }
        if !released {
            self.status.cleanup_failed();
        }
        self.status.lease_released(released);
        result
    }
}
