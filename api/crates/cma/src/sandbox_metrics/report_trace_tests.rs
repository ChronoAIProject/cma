use super::*;
use layer2_core::sandbox_metrics::MetricsError;
use std::sync::{Arc, Mutex};
use tracing::{instrument::WithSubscriber, Instrument};
#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn generation_trace_records_completion_failure_and_cancelled_stage_without_content() {
    for mode in 0..3 {
        let output = Output::default();
        let writer = output.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .finish();
        async {
            let operation = async {
                let mut trace = ReportTrace::new("private-report-id");
                trace
                    .stage(Stage::Inventory, async { Ok(()) })
                    .await
                    .unwrap();
                let result = trace
                    .stage(Stage::Allocation, async {
                        match mode {
                            0 => Ok(()),
                            1 => Err(MetricsError::Unavailable),
                            _ => std::future::pending().await,
                        }
                    })
                    .await;
                trace.finish(result.is_ok());
            };
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(5),
                operation.instrument(tracing::info_span!(
                    "private-span",
                    payload = "private-payload"
                )),
            )
            .await;
        }
        .with_subscriber(subscriber)
        .await;
        let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        let lines = text
            .lines()
            .filter(|l| l.contains("usage_report_generation_stages"))
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 1, "{text}");
        let line = lines[0];
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        let fields = &event["fields"];
        assert_eq!(fields["report_hash"], report_hash("private-report-id"));
        assert_eq!(fields["inventory_calls"], 1);
        assert_eq!(fields["allocation_calls"], 1);
        assert_eq!(fields["outcome"], ["complete", "failed", "cancelled"][mode]);
        assert_eq!(
            fields["active_stage"],
            if mode == 2 { "allocation" } else { "none" }
        );
        assert_eq!(event["span"]["name"], "usage_report_diagnostic");
        assert_eq!(event["spans"].as_array().unwrap().len(), 1);
        for private in [
            "private-report-id",
            "private-span",
            "private-payload",
            "payload=",
        ] {
            assert!(!line.contains(private), "{line}");
        }
    }
}
