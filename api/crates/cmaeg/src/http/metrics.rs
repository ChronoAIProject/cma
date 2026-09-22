//! Prometheus scrape surface; deliberately absent from the public router.
use axum::{extract::State, http::header, routing::get, Router};
use metrics_exporter_prometheus::PrometheusHandle;
use std::net::SocketAddr;
use tokio::task::JoinHandle;
pub struct MetricsServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}
impl MetricsServer {
    pub async fn start(
        address: SocketAddr,
        handle: PrometheusHandle,
    ) -> Result<Self, std::io::Error> {
        let listener = tokio::net::TcpListener::bind(address).await?;
        let address = listener.local_addr()?;
        let router = Router::new()
            .route("/metrics", get(scrape))
            .with_state(handle);
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, router).await {
                tracing::error!(%error, "metrics listener stopped");
            }
        });
        tracing::info!(%address, "CMAEG metrics listening");
        Ok(Self { address, task })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}
impl Drop for MetricsServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn scrape(
    State(handle): State<PrometheusHandle>,
) -> ([(header::HeaderName, &'static str); 2], String) {
    (
        [
            (
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        handle.render(),
    )
}
