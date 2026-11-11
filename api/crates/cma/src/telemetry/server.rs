//! Private pod scrape listener, absent from the public HTTP router/Service.
use axum::{extract::State, http::header, routing::get, Router};
use metrics_exporter_prometheus::PrometheusHandle;
use std::net::SocketAddr;
use tokio::task::JoinHandle;

pub(crate) struct Server(JoinHandle<()>);

impl Server {
    pub(crate) async fn start(
        address: SocketAddr,
        handle: PrometheusHandle,
    ) -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(|error| format!("cannot bind metrics listener {address}: {error}"))?;
        let router = router(handle);
        tracing::info!(address = %listener.local_addr().map_err(|error| error.to_string())?, "CMA metrics listening");
        Ok(Self(tokio::spawn(async move {
            // Loss of monitoring is reported by the external scrape/no-data
            // path; it must not interrupt application traffic or active work.
            if let Err(error) = axum::serve(listener, router).await {
                tracing::error!(%error, "CMA metrics listener stopped");
            }
        })))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn router(handle: PrometheusHandle) -> Router {
    Router::new()
        .route("/metrics", get(scrape))
        .with_state(handle)
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn scrape_router_has_only_metrics_and_does_not_cache() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let app = router(recorder.handle());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/plain"));
        assert_eq!(
            app.oneshot(
                Request::builder()
                    .uri("/api/v1/defaults")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
            StatusCode::NOT_FOUND
        );
    }
}
