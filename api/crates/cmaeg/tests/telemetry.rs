use cmaeg::{
    http::metrics::MetricsServer,
    telemetry::{self, names},
};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};
use tracing_subscriber::{layer::SubscriberExt, Layer};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Run `emit` under the gateway's log filter built from `env`, returning the
/// formatted lines.
fn captured(env: Option<&str>, emit: impl FnOnce()) -> String {
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .with_filter(cmaeg::log_layer_filter(env)),
    );
    tracing::subscriber::with_default(subscriber, emit);
    let logs = capture.0.lock().unwrap().clone();
    String::from_utf8(logs).unwrap()
}

#[test]
fn http_client_logs_are_capped_at_warn() {
    let logs = captured(
        Some("trace,reqwest::connect=trace,hyper_util::client=trace,h2=trace"),
        || {
            tracing::debug!(target: "reqwest::connect", "reqwest-connect-debug");
            tracing::debug!(target: "hyper_util::client::legacy", "hyper-util-debug");
            tracing::debug!(target: "h2::codec", "h2-codec-debug");
            tracing::debug!(target: "hyper", "hyper-debug");
            tracing::debug!(target: "cmaeg::media", "cmaeg-media-debug");
            tracing::warn!(target: "reqwest", "reqwest-warn");
        },
    );
    assert!(logs.contains("cmaeg-media-debug"), "{logs}");
    assert!(logs.contains("reqwest-warn"), "{logs}");
    for hidden in [
        "reqwest-connect-debug",
        "hyper-util-debug",
        "h2-codec-debug",
        "hyper-debug",
    ] {
        assert!(!logs.contains(hidden), "{hidden} leaked: {logs}");
    }
    assert_eq!(logs.lines().count(), 2, "{logs}");
}

#[test]
fn the_log_filter_falls_back_to_the_default_and_still_honours_lower_levels() {
    let emit = || {
        tracing::debug!(target: "cmaeg::media", "cmaeg-debug");
        tracing::info!(target: "cmaeg::media", "cmaeg-info");
        tracing::info!(target: "other_crate", "other-info");
    };
    for env in [None, Some("=[not a directive")] {
        let logs = captured(env, emit);
        assert!(logs.contains("cmaeg-info"), "{env:?}: {logs}");
        assert!(!logs.contains("cmaeg-debug"), "{env:?}: {logs}");
        assert!(!logs.contains("other-info"), "{env:?}: {logs}");
    }
    let logs = captured(Some("warn"), emit);
    assert!(logs.is_empty(), "{logs}");
}

#[tokio::test]
async fn metrics_register_once_and_curl_scrapes_the_private_listener() {
    let first = telemetry::install().unwrap();
    let second = telemetry::install().unwrap();
    metrics::histogram!(names::INGRESS_TO_PROVIDER, names::SOURCE => "custom", names::COLD => "false", names::MEDIA => "none").record(0.012);
    metrics::histogram!(names::MEDIA_FETCH_SECONDS, names::SOURCE => "nyxid", names::OUTCOME => "ok").record(0.2);
    let server = MetricsServer::start("127.0.0.1:0".parse().unwrap(), first)
        .await
        .unwrap();
    let output = tokio::process::Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "5",
            &format!("http://{}/metrics", server.address()),
        ])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    for name in names::HISTOGRAMS
        .into_iter()
        .chain(names::COUNTERS)
        .chain(names::GAUGES)
        .chain(names::MEDIA_HISTOGRAMS)
        .chain(names::MEDIA_COUNTERS)
        .chain(names::MEDIA_GAUGES)
    {
        assert_eq!(
            text.matches(&format!("# TYPE {name} ")).count(),
            1,
            "missing or duplicate metric {name}: {text}"
        );
    }
    assert!(text.contains("le=\"0.005\""));
    assert!(text.contains("le=\"10\""));
    // Media histograms use their own bucket set, reaching the 90 s budget;
    // every other histogram keeps the default set.
    assert!(text
        .contains("cmaeg_media_fetch_seconds_bucket{source=\"nyxid\",outcome=\"ok\",le=\"90\"} 1"));
    assert!(!text.contains(
        "cmaeg_media_fetch_seconds_bucket{source=\"nyxid\",outcome=\"ok\",le=\"0.005\"}"
    ));
    assert!(text.contains(
        "cmaeg_ingress_to_provider_seconds_bucket{source=\"custom\",cold=\"false\",media=\"none\",le=\"10\"} 1"
    ));
    assert!(!text.contains(
        "cmaeg_ingress_to_provider_seconds_bucket{source=\"custom\",cold=\"false\",media=\"none\",le=\"90\"}"
    ));
    for gauge in names::MEDIA_GAUGES {
        assert!(
            text.lines().any(|line| line == format!("{gauge} 0")),
            "{gauge}"
        );
    }
    let rendered = second.render();
    let mut first_lines: Vec<_> = text.lines().collect();
    let mut second_lines: Vec<_> = rendered.lines().collect();
    first_lines.sort_unstable();
    second_lines.sort_unstable();
    assert_eq!(first_lines, second_lines);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let public = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, cmaeg::router()).await.unwrap();
    });
    let status = reqwest::get(format!("http://{public}/metrics"))
        .await
        .unwrap()
        .status();
    task.abort();
    assert_eq!(status, 404);
}
