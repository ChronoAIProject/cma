//! Private pod scrape listener, absent from the public HTTP router/Service.
use axum::{extract::State, http::header, routing::get, Router};
use metrics_exporter_prometheus::PrometheusHandle;
use std::net::SocketAddr;
use tokio::task::JoinHandle;

pub(crate) struct Server {
    listener: JoinHandle<()>,
    upkeep: JoinHandle<()>,
    runtime: JoinHandle<()>,
}

impl Server {
    pub(crate) async fn start(
        address: SocketAddr,
        handle: PrometheusHandle,
        cohorts: super::task_cohort::Cache,
    ) -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(|error| format!("cannot bind metrics listener {address}: {error}"))?;
        let router = router(handle.clone(), cohorts);
        tracing::info!(address = %listener.local_addr().map_err(|error| error.to_string())?, "CMA metrics listening");
        // build_recorder does not spawn upkeep. Drain raw histogram samples
        // even if no collector scrapes us or the listener later fails.
        let upkeep = tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                handle.run_upkeep();
            }
        });
        let listener = tokio::spawn(async move {
            // Loss of monitoring is reported by the external scrape/no-data
            // path; it must not interrupt application traffic or active work.
            if let Err(error) = axum::serve(listener, router).await {
                tracing::error!(%error, "CMA metrics listener stopped");
            }
        });
        let runtime = super::runtime::spawn();
        Ok(Self {
            listener,
            upkeep,
            runtime,
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.listener.abort();
        self.upkeep.abort();
        self.runtime.abort();
    }
}

fn router(handle: PrometheusHandle, cohorts: super::task_cohort::Cache) -> Router {
    Router::new()
        .route("/metrics", get(scrape))
        .with_state((handle, cohorts))
}

async fn scrape(
    State((handle, cohorts)): State<(PrometheusHandle, super::task_cohort::Cache)>,
) -> ([(header::HeaderName, &'static str); 2], String) {
    let mut output = handle.render();
    layer1_public::desktop_telemetry::append_desktop_send_histograms(&mut output);
    output.push_str(&cohorts.render());
    (
        [
            (
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        output,
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
    async fn dropping_server_stops_listener_and_scrape_independent_upkeep() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let server = Server::start(
            "127.0.0.1:0".parse().unwrap(),
            recorder.handle(),
            Default::default(),
        )
        .await
        .unwrap();
        let listener = server.listener.abort_handle();
        let upkeep = server.upkeep.abort_handle();
        let runtime = server.runtime.abort_handle();
        tokio::task::yield_now().await;
        assert!(!listener.is_finished());
        assert!(!upkeep.is_finished());
        assert!(!runtime.is_finished());
        // Losing the listener must not stop local sampling or histogram upkeep.
        listener.abort();
        tokio::task::yield_now().await;
        assert!(!runtime.is_finished());
        assert!(!upkeep.is_finished());
        drop(server);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !listener.is_finished() || !upkeep.is_finished() || !runtime.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("metrics tasks must stop when their guard is dropped");
    }

    #[tokio::test]
    async fn scrape_router_has_only_metrics_and_does_not_cache() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let cache = super::super::task_cohort::Cache::default();
        cache.initialize();
        cache.update(Ok(
            "cma_task_cohort_accepted_tasks{window=\"release\"} 2\n".into()
        ));
        let app = router(recorder.handle(), cache);
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
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let text = std::str::from_utf8(&body).unwrap();
        assert!(text.contains("cma_task_cohort_accepted_tasks{window=\"release\"} 2"));
        assert_eq!(
            text.matches("# TYPE cma_desktop_relay_send_wait_seconds histogram")
                .count(),
            1
        );
        assert!(text.contains("cma_desktop_relay_send_wait_seconds_count{direction=\"uplink\"}"));
        assert!(text.contains("cma_desktop_relay_send_wait_seconds_count{direction=\"downlink\"}"));
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
