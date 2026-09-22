#[path = "fake_provider.rs"]
mod fake_provider;
#[path = "turns/support.rs"]
mod support;

use cmaeg::ingress::launch::TurnLauncher;
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};
use support::Fixture;
#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn socket_metric_is_under_twenty_ms_and_logs_no_secrets() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let mut fixture = Fixture::new(Default::default(), Default::default(), 4).await;
    let capture = Capture(Default::default());
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let mut request = fixture.request("metric");
    let timing = cmaeg::telemetry::timing::IngressTiming::new("custom");
    timing.received();
    request.timing = Some(timing.clone());
    let reference = request.event_ref.as_str().to_owned();
    fixture.runner.launch(request).await;
    fixture.terminal().await;
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    let scrape = metrics.render();
    let line = scrape
        .lines()
        .find(|line| line.starts_with("cmaeg_ingress_to_provider_seconds_sum{"))
        .unwrap();
    let seconds: f64 = line.split_whitespace().last().unwrap().parse().unwrap();
    assert!(seconds < 0.020, "{logs}");
    assert!(
        logs.contains("cmaeg_provider_accept_seconds"),
        "missing acceptance metric: {logs}"
    );
    assert!(!logs.contains(&reference));
    assert!(!logs.contains("runtime-test-provider-bearer"));
    assert!(fixture.readiness.report().ready);
    assert!(
        fixture
            .tracker
            .drain(std::time::Instant::now() + std::time::Duration::from_secs(1))
            .await
    );
    assert!(!timing.header().contains("not_written"));
    let scrape = metrics.render();
    assert!(
        scrape.lines().any(
            |line| line.starts_with("cmaeg_ingress_to_provider_seconds_count{")
                && line.contains("source=\"custom\"")
                && line.ends_with(" 1")
        ),
        "{scrape}"
    );
    assert!(scrape.contains("cmaeg_turns_inflight 0"));
    assert!(scrape.lines().any(
        |line| line.starts_with("cmaeg_turn_duration_seconds_count{")
            && line.contains("completed")
            && line.ends_with(" 1")
    ));
    assert_eq!(fixture.fake.requests().len(), 1);
    fixture.fake.close().await;
}
