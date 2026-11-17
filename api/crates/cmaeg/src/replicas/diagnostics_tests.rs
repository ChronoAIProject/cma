use super::*;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn diagnostics_are_typed_hashed_throttled_and_gauge_recovers() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    metrics::with_local_recorder(&recorder, || {
        runtime.block_on(async {
            let mut isolation = Isolation::default();
            let error = ScanError::new(
                Class::MissingPlan,
                Stage::PlanLoad,
                Some("private-partition-content"),
            );
            assert_eq!(error.partition_hash.len(), 12);
            assert!(error.partition_hash.bytes().all(|c| c.is_ascii_hexdigit()));
            isolation.failed("private-partition-content", &error);
            assert!(!isolation.ready("private-partition-content"));
            assert!(handle.render().contains("cmaeg_replica_isolated_heads 1"));
            isolation.failed("private-partition-content", &error);
            isolation.healthy("private-partition-content");
            isolation.failed("private-partition-content", &error);
            assert_eq!(
                String::from_utf8(capture.0.lock().unwrap().clone())
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
            tokio::time::advance(Duration::from_secs(181)).await;
            isolation.failed("private-partition-content", &error);
            let log = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
            assert_eq!(log.lines().count(), 2);
            for field in [
                "class=\"missing_plan\"",
                "stage=\"plan_load\"",
                &error.partition_hash,
            ] {
                assert!(log.contains(field), "{log}");
            }
            assert!(!log.contains("private-partition-content"));
            let text = handle.render();
            assert!(text.contains("class=\"missing_plan\""));
            assert!(text.contains("stage=\"plan_load\""));
            assert!(!text.contains("partition_hash"));
            assert!(!text.contains("private-partition-content"));
            isolation.retain(&BTreeSet::new());
            assert!(handle.render().contains("cmaeg_replica_isolated_heads 0"));
        })
    });
}

#[tokio::test(start_paused = true)]
async fn transient_retry_delay_is_per_head_and_bounded() {
    let mut isolation = Isolation::default();
    let error = ScanError::new(Error::Unavailable, Stage::ItemRead, Some("failing-head"));
    for attempt in 0..10 {
        isolation.failed("failing-head", &error);
        assert!(!isolation.ready("failing-head"));
        assert!(isolation.ready("healthy-head"));
        let delay = (1u64 << attempt).min(60);
        tokio::time::advance(Duration::from_secs(delay)).await;
        assert!(isolation.ready("failing-head"));
    }
    isolation.healthy("failing-head");
    assert!(isolation.ready("failing-head"));
}
