use cmaeg::{
    http::metrics::MetricsServer,
    telemetry::{self, names},
};

#[tokio::test]
async fn metrics_register_once_and_curl_scrapes_the_private_listener() {
    let first = telemetry::install().unwrap();
    let second = telemetry::install().unwrap();
    metrics::histogram!(names::INGRESS_TO_PROVIDER, names::SOURCE => "custom", names::COLD => "false").record(0.012);
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
    assert!(!text.contains(
        "cmaeg_ingress_to_provider_seconds_bucket{source=\"custom\",cold=\"false\",le=\"90\"}"
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
