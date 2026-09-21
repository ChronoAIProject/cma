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
async fn fake_dispatch_metric_is_under_twenty_ms_and_logs_no_secrets() {
    let mut fixture = Fixture::new(Default::default(), Default::default(), 4).await;
    let capture = Capture(Default::default());
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let request = fixture.request("metric");
    let reference = request.event_ref.as_str().to_owned();
    fixture.runner.launch(request).await;
    fixture.terminal().await;
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    let line = logs
        .lines()
        .find(|line| line.contains("cmaeg_ingress_to_provider_seconds"))
        .unwrap_or_else(|| panic!("missing ingress metric: {logs}"));
    let seconds: f64 = line
        .split("duration_seconds=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
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
    assert_eq!(fixture.fake.requests().len(), 1);
    fixture.fake.close().await;
}
