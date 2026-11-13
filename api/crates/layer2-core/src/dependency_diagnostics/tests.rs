use super::*;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

#[tokio::test]
async fn attribution_is_future_local_and_first_failure_wins() {
    let runtime = Failure::new(Dependency::Runtime, Reason::Unavailable);
    let storage = Failure::new(Dependency::Storage, Reason::Corrupt);
    let (first, second) = tokio::join!(
        capture(async {
            runtime.record();
            tokio::task::yield_now().await;
            storage.record();
            assert_eq!(
                capture(async {
                    storage.record();
                })
                .await
                .1,
                Some(storage)
            );
            1
        }),
        capture(async {
            storage.record();
            tokio::task::yield_now().await;
            2
        })
    );
    assert_eq!(first, (1, Some(runtime)));
    assert_eq!(second, (2, Some(storage)));
    runtime.record(); // No active capture: no effect on a later request.
    assert_eq!(capture(async { 3 }).await, (3, None));
}

#[tokio::test]
async fn cancellation_does_not_leak_attribution_to_next_request() {
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(1),
        capture(async {
            Failure::new(Dependency::Storage, Reason::Unavailable).record();
            std::future::pending::<()>().await;
        }),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(capture(async {}).await.1, None);
}

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[test]
fn counters_saturate_and_logs_exclude_parent_span_fields() {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let counter = AtomicU64::new(0);
    let span = tracing::info_span!(
        "private_request",
        path = "private fixture path",
        subject = "private fixture subject"
    );
    let _entered = span.enter();
    let failure = Failure::new(Dependency::Runtime, Reason::RuntimeLedgerHold);
    emit(&counter, Operation::FileContent, failure);
    emit(&counter, Operation::FileContent, failure);
    assert_eq!(counter.load(Ordering::Relaxed), 2);
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.lines().count(), 2);
    for label in [
        "cma_dependency_failures_total",
        "file_content",
        "runtime_ledger_hold",
        "failures_total=1",
        "failures_total=2",
    ] {
        assert!(logs.contains(label), "{logs}");
    }
    for private in ["private", "path=", "subject="] {
        assert!(!logs.contains(private), "{logs}");
    }
    counter.store(u64::MAX - 1, Ordering::Relaxed);
    for _ in 0..3 {
        emit(&counter, Operation::FileContent, failure);
    }
    assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
}
