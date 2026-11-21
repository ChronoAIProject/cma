//! Inject only an established HTTP stream; tests still execute real checkout,
//! native authority reads, HTTP encoding, correlation and cancellation.
use super::*;
use hyper_util::rt::TokioIo;

impl ProviderHostedControlTransport {
    pub(crate) async fn seed_native_status_connection(
        &self,
        handle: &RuntimeHandle,
    ) -> tokio::io::DuplexStream {
        self.seed_native_connection(handle, native_pool::NativeHttpLane::Control)
            .await
            .0
    }

    pub(crate) async fn seed_native_ownership_connection(
        &self,
        handle: &RuntimeHandle,
    ) -> (tokio::io::DuplexStream, tokio::sync::watch::Receiver<bool>) {
        self.seed_native_connection(handle, native_pool::NativeHttpLane::Ownership)
            .await
    }

    async fn seed_native_connection(
        &self,
        handle: &RuntimeHandle,
        lane: native_pool::NativeHttpLane,
    ) -> (tokio::io::DuplexStream, tokio::sync::watch::Receiver<bool>) {
        let endpoint = self.endpoint(handle).await.unwrap();
        let slot = self.native_slot_for(&endpoint, lane).await.unwrap();
        let cancelled = slot.cancellation();
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
        (server, cancelled)
    }
}

tokio::task_local! {
    static STATUS_BOUNDARY_HOOK: Box<dyn Fn(bool) + Send + Sync>;
}

impl ProviderHostedControlTransport {
    pub(crate) async fn with_status_hook<F: std::future::Future>(
        hook: impl Fn(bool) + Send + Sync + 'static,
        operation: F,
    ) -> F::Output {
        STATUS_BOUNDARY_HOOK.scope(Box::new(hook), operation).await
    }
    pub(crate) fn test_status_boundary(response_captured: bool) {
        let _ = STATUS_BOUNDARY_HOOK.try_with(|hook| hook(response_captured));
    }
}
