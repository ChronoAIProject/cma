use super::*;
use crate::sandbox_usage_export::{Cursor, Receipt, TransportError, UsageExporter, UsageTransport};
use crate::store::sandbox_usage_export::FrozenUsageEvent;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Inbox {
    receipts: Mutex<BTreeMap<String, Receipt>>,
    submitted: Mutex<Vec<Vec<u8>>>,
}
#[async_trait::async_trait]
impl UsageTransport for Inbox {
    async fn submit(&self, event: &FrozenUsageEvent) -> Result<Receipt, TransportError> {
        let receipt=serde_json::from_value(serde_json::json!({"receipt_id":uuid::Uuid::new_v4().to_string(),"event_id":event.event_id,
            "stream_id":event.stream_id,"sequence":event.sequence.to_string(),"payload_digest":event.digest,"accepted_at_ms":event.produced_at_ms.to_string(),"custody":"persisted"})).unwrap();
        self.receipts
            .lock()
            .unwrap()
            .insert(event.event_id.clone(), receipt);
        self.submitted.lock().unwrap().push(event.payload.clone());
        Err(TransportError::Unavailable)
    }
    async fn receipt(&self, id: &str) -> Result<Option<Receipt>, TransportError> {
        Ok(self.receipts.lock().unwrap().get(id).cloned())
    }
    async fn cursor(&self, _: &str) -> Result<Option<Cursor>, TransportError> {
        Ok(None)
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn shared_sender_discovers_measured_backlog_after_capture_stops_and_recovers_lost_reply() {
    let f = Fixture::new().await;
    f.bind().await;
    let lease = f.lease().await;
    let original = f.input(&lease).await.freeze(f.samples(0.0), true).unwrap();
    f.store.publish_measured(&lease, &original).await.unwrap();
    f.store.release_measured(&lease).await.unwrap();
    let lease = f.lease().await;
    let correction = f.input(&lease).await.freeze(f.samples(60.0), true).unwrap();
    f.store.publish_measured(&lease, &correction).await.unwrap();
    f.store.release_measured(&lease).await.unwrap();
    let originals = f.rows(OUTBOX).await;
    let reopened = CaptureStore::open(&f.store.client, f.db.name(), f.store.config.clone())
        .await
        .unwrap();
    let inbox = Arc::new(Inbox::default());
    let worker = UsageExporter::for_test(reopened, "a".repeat(64), inbox.clone());
    let mut after = String::new();
    for _ in 0..16 {
        worker.sweep(&mut after).await.unwrap();
        f.store
            .collection(OUTBOX)
            .update_many(
                doc! {"delivery.status":"pending"},
                doc! {"$set":{"delivery.due_at":mongodb::bson::DateTime::from_millis(0)}},
            )
            .await
            .unwrap();
    }
    assert_eq!(
        inbox.submitted.lock().unwrap().len(),
        3,
        "receipt recovery must never submit again"
    );
    let final_rows = f.rows(OUTBOX).await;
    assert_eq!(final_rows.len(), 3);
    for row in final_rows {
        assert_eq!(
            row.get_document("delivery")
                .unwrap()
                .get_str("status")
                .unwrap(),
            "accepted"
        );
        let original = originals
            .iter()
            .find(|old| old.get("_id") == row.get("_id"))
            .unwrap();
        assert_eq!(row.get("payload"), original.get("payload"));
        assert_eq!(row.get("digest"), original.get("digest"));
    }
    f.db.drop().await.unwrap();
}
