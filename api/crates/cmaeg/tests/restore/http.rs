use super::*;
use axum::{
    extract::{Path, State},
    routing::get,
    Router,
};
use cmaeg::{
    channels::resolver::ChannelResolver,
    http::{
        problems::{problem, protocol_headers},
        readiness::readyz,
    },
};
use std::{io::Write, sync::Mutex};
use tokio::sync::oneshot;
use tracing::instrument::WithSubscriber;

struct LogWriter(Arc<Mutex<Vec<u8>>>);
impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn one_hundred_http_misses_emit_one_restore_span() {
    let mut fixture = Fixture::new(ChannelCacheConfig::default()).await;
    *fixture.server.state.record.lock().unwrap() = None;
    fixture.server.state.delay_ms.store(30, Ordering::SeqCst);
    let logs = Arc::new(Mutex::new(Vec::new()));
    let output = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NEW)
        .with_writer(move || LogWriter(output.clone()))
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    // Other tests also register this callsite; a global collector avoids racing
    // the global maximum-level hint against their subscriber-free threads.
    tracing::dispatcher::set_global_default(dispatch.clone()).unwrap();
    // A test-only projection exercises the resolver before the ingress routes are introduced.
    let router = Router::new()
        .route(
            "/resolve/{id}",
            get(
                move |State(resolver): State<ChannelResolver>, Path(id): Path<String>| {
                    let dispatch = dispatch.clone();
                    async move {
                        let error = resolver.resolve(&id).await.unwrap_err().problem();
                        problem(
                            axum::http::StatusCode::from_u16(error.status).unwrap(),
                            error.code,
                            "channel not found",
                        )
                    }
                    .with_subscriber(dispatch)
                },
            ),
        )
        .with_state(fixture.resolver.clone())
        .layer(axum::middleware::from_fn(protocol_headers));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/resolve/{}",
        listener.local_addr().unwrap(),
        fixture.id
    );
    let (stop, signal) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = signal.await;
            })
            .await
            .unwrap();
    });
    let client = reqwest::Client::new();
    let responses = join_all((0..100).map(|_| client.get(&url).send())).await;
    for response in responses {
        assert_eq!(response.unwrap().status(), 404);
    }
    assert_eq!(fixture.server.count(), 1);
    let trace = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert_eq!(
        trace
            .lines()
            .filter(|line| line.contains("cmaeg.restore")
                && line.contains(": new")
                && line.contains(&cmaeg::telemetry::channel_hash(fixture.id.as_str())))
            .count(),
        1,
        "{trace}"
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.server.stop().await;
}

async fn curl_reachable(url: &str, expected: bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    loop {
        let output = tokio::process::Command::new("curl")
            .args(["--silent", "--show-error", "--fail", "--max-time", "2", url])
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["ready"], true);
        if report["cma_reachable"] == expected {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "probe did not observe reachability change"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn curl_reports_background_cma_reachability_after_stop_and_restart() {
    let mut fixture = Fixture::new(ChannelCacheConfig::default()).await;
    let readiness = Readiness::initialized();
    let _probe = readiness.start_probe(fixture.records.clone());
    let router = Router::new()
        .route("/readyz", get(readyz))
        .with_state(readiness)
        .layer(axum::middleware::from_fn(protocol_headers));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/readyz", listener.local_addr().unwrap());
    let (stop, signal) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = signal.await;
            })
            .await
            .unwrap();
    });
    curl_reachable(&url, true).await;
    fixture.server.stop().await;
    curl_reachable(&url, false).await;
    fixture.server.restart().await;
    curl_reachable(&url, true).await;
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.server.stop().await;
}
