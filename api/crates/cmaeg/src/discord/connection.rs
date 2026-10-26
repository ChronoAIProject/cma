//! One configured, owned connection with lease renewal. No automatic activation.
use super::{DurableJournal, EndReason, GatewayClient, GatewayEnd, GatewayError};
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
        let gateway = GatewayClient::new(
            self.authority.token.clone(),
            self.authority.bot_id.clone(),
            self.authority.intents,
        )?;
        let resume = self.checkpoint().await?;
        let (stop, receiver) = watch::channel(*shutdown.borrow());
        let connection = gateway.connect(resume, self.clone(), receiver);
        tokio::pin!(connection);
        let renewal = async {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                if tokio::time::timeout(Duration::from_secs(15), self.renew()).await != Ok(Ok(())) {
                    break;
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
        if result.as_ref().is_ok_and(|end| end.checkpoint.is_none()) {
            let _ = self.discard_checkpoint().await;
        }
        // Failed release is safe: a new process must wait for the durable lease
        // to expire, and stale workers cannot release a newer owner's fence.
        let _ = self.release().await;
        result
    }
}
