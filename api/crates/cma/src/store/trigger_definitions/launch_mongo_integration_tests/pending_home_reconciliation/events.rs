use super::*;

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Events {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Events {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn drift_and_missing_launches_emit_structured_operations_events() {
    use tracing::instrument::WithSubscriber;
    let (seed, _, reconciler) = setup("pending_events").await;
    admissions(&seed)
        .insert_one(
            doc! {"_id":{"scope":"launch_home_capacity","launch":"missing-event-launch"},
            "document_schema_version":1_i64,"owner":"owner","state":"reserved"},
        )
        .await
        .unwrap();
    let output = Events::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let report = reconciler
        .sweep(None)
        .with_subscriber(subscriber)
        .await
        .unwrap();
    assert_eq!(report.missing_launches, 1);
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let drift = events
        .iter()
        .find(|event| event["fields"]["event"] == "pending_launch_homes_drift")
        .unwrap();
    assert_eq!(drift["fields"]["pending_launch_homes"], 0);
    assert_eq!(drift["fields"]["expected_pending_launch_homes"], 1);
    assert_eq!(drift["fields"]["pending_launch_homes_drift"], -1);
    assert!(events
        .iter()
        .any(|event| event["fields"]["event"] == "pending_launch_home_missing_launch"));
    seed.client.database(&seed.database).drop().await.unwrap();
}
