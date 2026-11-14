use super::*;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

#[tokio::test(start_paused = true)]
async fn phase_observation_preserves_results_and_records_cancellation() {
    let output = Output::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || writer.clone())
        .finish();
    let _scope = tracing::subscriber::set_default(subscriber);
    assert_eq!(observe("success", async { Ok::<_, ()>(7) }).await, Ok(7));
    assert_eq!(observe("failure", async { Err::<(), _>(9) }).await, Err(9));
    assert!(tokio::time::timeout(
        Duration::from_millis(10),
        observe("cancelled", std::future::pending::<Result<(), ()>>()),
    )
    .await
    .is_err());
    let bytes = output.0.lock().unwrap();
    let records: Vec<serde_json::Value> = std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 3);
    for (record, expected) in records.iter().zip(["success", "failure", "cancelled"]) {
        assert_eq!(record["target"], "cma::desktop::connect");
        assert_eq!(record["fields"]["outcome"], expected);
        assert_eq!(record["fields"]["phase"], expected);
        assert_eq!(record["fields"].as_object().unwrap().len(), 4);
    }
    assert_eq!(records[2]["fields"]["elapsed_ms"], 10.0);
}
