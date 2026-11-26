//! Prometheus scrape surface; deliberately absent from the public router.
use crate::telemetry::provider_start::Measurement;
use axum::{extract::State, http::header, routing::get, Router};
use metrics_exporter_prometheus::PrometheusHandle;
use std::net::SocketAddr;
use tokio::task::JoinHandle;
#[derive(Clone)]
struct MetricsState {
    handle: PrometheusHandle,
    provider_start: Option<Measurement>,
}

pub struct MetricsServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}
impl MetricsServer {
    pub async fn start(
        address: SocketAddr,
        handle: PrometheusHandle,
    ) -> Result<Self, std::io::Error> {
        Self::start_with_provider_start(address, handle, None).await
    }

    pub(crate) async fn start_with_provider_start(
        address: SocketAddr,
        handle: PrometheusHandle,
        provider_start: Option<Measurement>,
    ) -> Result<Self, std::io::Error> {
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(|error| {
                std::io::Error::new(error.kind(), format!("metrics listener {address}: {error}"))
            })?;
        let address = listener.local_addr()?;
        let router = Router::new()
            .route("/metrics", get(scrape))
            .with_state(MetricsState {
                handle,
                provider_start,
            });
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
    State(state): State<MetricsState>,
) -> ([(header::HeaderName, &'static str); 2], String) {
    if let Some(measurement) = &state.provider_start {
        measurement.record_window_metrics();
    }
    (
        [
            (
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        state.handle.render(),
    )
}
