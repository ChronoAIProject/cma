use super::*;
use layer1_public::rest::admin::warm_pools::WarmPoolError;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

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

/// Run the gate on this thread and return its result with the one phase row it emitted.
async fn gate(
    wait: Result<StartupReadiness, WarmPoolError>,
) -> (Result<StartupReadiness, String>, String) {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(output.clone())
        .finish();
    let result = {
        let _default = tracing::subscriber::set_default(subscriber);
        default_pool_gate(async { wait }).await
    };
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let rows: Vec<_> = text
        .lines()
        .filter(|row| row.contains("phase=\"default_warm_pool_readiness\""))
        .collect();
    assert_eq!(rows.len(), 1, "{text}");
    (result, rows[0].to_owned())
}

#[tokio::test]
async fn startup_gate_continues_serving_when_the_default_pool_bound_elapses() {
    let (result, row) = gate(Ok(StartupReadiness::Degraded)).await;
    assert_eq!(result, Ok(StartupReadiness::Degraded));
    assert!(row.contains("outcome=\"degraded\""), "{row}");
}

#[tokio::test]
async fn startup_gate_reports_ready_capacity_as_ok() {
    let (result, row) = gate(Ok(StartupReadiness::Ready)).await;
    assert_eq!(result, Ok(StartupReadiness::Ready));
    assert!(row.contains("outcome=\"ok\""), "{row}");
}

#[tokio::test]
async fn startup_gate_still_stops_on_a_retiring_or_unusable_default() {
    for error in [WarmPoolError::Conflict, WarmPoolError::Unavailable] {
        let (result, row) = gate(Err(error)).await;
        let message = result.unwrap_err();
        assert!(
            message.starts_with("native warm pool startup readiness failed:"),
            "{message}"
        );
        assert!(row.contains("outcome=\"error\""), "{row}");
    }
}
