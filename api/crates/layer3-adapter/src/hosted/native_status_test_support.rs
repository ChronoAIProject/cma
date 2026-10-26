//! Inject only an established HTTP stream; tests still execute real checkout,
//! native authority reads, HTTP encoding, correlation and cancellation.
use super::*;
use hyper_util::rt::TokioIo;

impl ProviderHostedControlTransport {
    pub(crate) async fn seed_native_status_connection(
        &self,
        handle: &RuntimeHandle,
    ) -> tokio::io::DuplexStream {
        let endpoint = self.endpoint(handle).await.unwrap();
        let slot = self.native_slot(&endpoint).await.unwrap();
        let mut lease = self.native_checkout(&endpoint, slot).await.unwrap();
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(client))
            .await
            .unwrap();
        lease.connection = Some(native_pool::NativeConnection {
            sender,
            _driver: native_pool::AbortOnDrop(tokio::spawn(async move {
                let _ = connection.await;
            })),
        });
        lease.recycle().await.unwrap();
        server
    }
}
