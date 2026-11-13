use super::*;
use std::io::Write;
use std::sync::{Arc, Mutex};

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
fn counts_only_holds_with_unknown_occupancy_and_no_caller_payload() {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let counter = AtomicU64::new(0);
    let private = "private fixture must not be logged";
    let span = tracing::info_span!("caller", sandbox_id = private);
    let _entered = span.enter();
    record(
        &counter,
        &WorkerFailure::retryable("agent_cleanup_unavailable", private),
    );
    assert_eq!(counter.load(Ordering::Relaxed), 0);
    assert!(output.0.lock().unwrap().is_empty());
    for _ in 0..2 {
        record(
            &counter,
            &WorkerFailure::retryable(
                layer2_core::sandbox::store::AGENT_RUNTIME_LEDGER_HOLD,
                private,
            ),
        );
    }
    assert_eq!(counter.load(Ordering::Relaxed), 2);
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.lines().count(), 2);
    for text in [
        "runtime_ledger_capacity",
        "failures_total=1",
        "failures_total=2",
        "occupancy=\"unknown\"",
        "occupancy_support=\"unknown\"",
    ] {
        assert!(logs.contains(text), "{logs}");
    }
    assert!(!logs.contains(private));
    assert!(!logs.contains("sandbox_id"));
}

#[test]
fn counter_saturates_without_changing_failure_or_retry() {
    let counter = AtomicU64::new(u64::MAX - 1);
    let failure = WorkerFailure::retryable(
        layer2_core::sandbox::store::AGENT_RUNTIME_LEDGER_HOLD,
        "capacity",
    );
    for _ in 0..3 {
        record(&counter, &failure);
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
        assert!(failure.is_ledger_hold());
    }
}
