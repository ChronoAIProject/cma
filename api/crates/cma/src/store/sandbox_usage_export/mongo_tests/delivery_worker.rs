use super::*;
use crate::sandbox_usage_export::{
    Cursor, MissingRange, Receipt, TransportError, UsageExporter, UsageTransport,
};

mod compatibility;
use serde_json::json;

struct Inbox {
    receipts: Collection<Document>,
    submissions: Mutex<Vec<(String, Vec<u8>)>>,
    fail_stream: Mutex<Option<String>>,
    lose_reply: bool,
    cursor: Mutex<Option<Cursor>>,
}

impl Inbox {
    fn new(f: &Fixture, lose_reply: bool) -> Arc<Self> {
        Arc::new(Self {
            receipts: crate::store::durability::majority_collection(&f.db, "fixture_inbox"),
            submissions: Default::default(),
            fail_stream: Default::default(),
            lose_reply,
            cursor: Default::default(),
        })
    }
}

#[async_trait::async_trait]
impl UsageTransport for Inbox {
    async fn submit(&self, event: &FrozenUsageEvent) -> Result<Receipt, TransportError> {
        self.submissions
            .lock()
            .unwrap()
            .push((event.event_id.clone(), event.payload.clone()));
        if self.fail_stream.lock().unwrap().as_ref() == Some(&event.stream_id) {
            return Err(TransportError::RateLimited(300));
        }
        let receipt: Receipt = serde_json::from_value(json!({"receipt_id":uuid::Uuid::new_v4().to_string(),
            "event_id":event.event_id,"stream_id":event.stream_id,"sequence":event.sequence.to_string(),
            "payload_digest":event.digest,"accepted_at_ms":event.produced_at_ms.to_string(),"custody":"persisted"})).unwrap();
        self.receipts.insert_one(doc! {"_id":&event.event_id,"receipt":mongodb::bson::serialize_to_bson(&receipt).unwrap()})
            .await.unwrap();
        if self.lose_reply {
            Err(TransportError::Unavailable)
        } else {
            Ok(receipt)
        }
    }
    async fn receipt(&self, event_id: &str) -> Result<Option<Receipt>, TransportError> {
        Ok(self
            .receipts
            .find_one(doc! {"_id":event_id})
            .await
            .unwrap()
            .map(|row| {
                mongodb::bson::deserialize_from_bson(row.get("receipt").unwrap().clone()).unwrap()
            }))
    }
    async fn cursor(&self, _: &str) -> Result<Option<Cursor>, TransportError> {
        Ok(self.cursor.lock().unwrap().clone())
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn lost_reply_after_majority_receipt_recovers_after_restart_without_second_submit() {
    let f = Fixture::new(true).await;
    delivery::stream(&f, "not-enrolled-owner").await;
    let inbox = Inbox::new(&f, true);
    let worker = UsageExporter::for_test(f.capture(), "a".repeat(64), inbox.clone());
    worker.sweep(&mut String::new()).await.unwrap();
    assert_eq!(inbox.submissions.lock().unwrap().len(), 1);
    let row =
        f.db.collection::<Document>(OUTBOX)
            .find_one(doc! {"sequence":1_i64})
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        row.get_document("delivery")
            .unwrap()
            .get_str("status")
            .unwrap(),
        "pending"
    );
    f.db.collection::<Document>(OUTBOX)
        .update_one(
            doc! {"_id":row.get_str("_id").unwrap()},
            doc! {"$set":{"delivery.due_at":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let peer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let reopened = CaptureStore::open(&peer, f.db.name(), config())
        .await
        .unwrap();
    let recovered = Inbox::new(&f, false);
    UsageExporter::for_test(reopened, "a".repeat(64), recovered.clone())
        .sweep(&mut String::new())
        .await
        .unwrap();
    assert!(recovered.submissions.lock().unwrap().is_empty());
    let accepted =
        f.db.collection::<Document>(OUTBOX)
            .find_one(doc! {"_id":row.get_str("_id").unwrap()})
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        accepted
            .get_document("delivery")
            .unwrap()
            .get_str("status")
            .unwrap(),
        "accepted"
    );
    assert_eq!(row.get("payload"), accepted.get("payload"));
    assert_eq!(f.count(OUTBOX).await, 2);
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn an_owner_retry_does_not_starve_other_streams_and_export_off_performs_no_io() {
    let f = Fixture::new(true).await;
    let failing = delivery::stream(&f, "not-enrolled").await;
    delivery::stream(&f, "zero-balance").await;
    delivery::stream(&f, "deleted-owner").await;
    let count = f.commands.lock().unwrap().len();
    assert!(UsageExporter::from_config(&f.store, None)
        .unwrap()
        .is_none());
    assert_eq!(f.commands.lock().unwrap().len(), count);
    let inbox = Inbox::new(&f, false);
    *inbox.fail_stream.lock().unwrap() = Some(failing);
    let worker = UsageExporter::for_test(f.capture(), "a".repeat(64), inbox.clone());
    let mut after = String::new();
    worker.sweep(&mut after).await.unwrap();
    assert_eq!(inbox.submissions.lock().unwrap().len(), 3);
    assert_eq!(inbox.receipts.count_documents(doc! {}).await.unwrap(), 2);
    worker.sweep(&mut after).await.unwrap();
    assert!(after.is_empty());
    worker.sweep(&mut after).await.unwrap();
    assert_eq!(inbox.submissions.lock().unwrap().len(), 5);
    assert_eq!(inbox.receipts.count_documents(doc! {}).await.unwrap(), 4);
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn remote_maximum_and_holes_cannot_acknowledge_unreceipted_events() {
    let f = Fixture::new(true).await;
    let stream = delivery::stream(&f, "free-owner").await;
    let inbox = Inbox::new(&f, false);
    *inbox.cursor.lock().unwrap() = Some(observed_cursor(&stream, 0, None));
    UsageExporter::for_test(f.capture(), "a".repeat(64), inbox.clone())
        .sweep(&mut String::new())
        .await
        .unwrap();
    assert_eq!(inbox.submissions.lock().unwrap().len(), 1);
    assert_eq!(inbox.receipts.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        f.db.collection::<Document>(OUTBOX)
            .count_documents(doc! {"delivery.status":"accepted"})
            .await
            .unwrap(),
        1
    );
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn acknowledged_streams_are_reconciled_and_remote_holes_preserve_receipts() {
    let f = Fixture::new(true).await;
    let stream = delivery::stream(&f, "free-owner").await;
    let inbox = Inbox::new(&f, false);
    let worker = UsageExporter::for_test(f.capture(), "a".repeat(64), inbox.clone());
    worker.sweep(&mut String::new()).await.unwrap();
    worker.sweep(&mut String::new()).await.unwrap();
    let rows = f.db.collection::<Document>(OUTBOX);
    let last = rows
        .find_one(doc! {"sequence":2_i64})
        .await
        .unwrap()
        .unwrap();
    let filter = doc! {"_id":last.get_str("_id").unwrap()};
    let due = doc! {"$set":{"delivery.due_at":mongodb::bson::DateTime::from_millis(0)}};
    *inbox.cursor.lock().unwrap() = Some(observed_cursor(
        &stream,
        2,
        Some(last.get_str("digest").unwrap().into()),
    ));
    rows.update_one(filter.clone(), due.clone()).await.unwrap();
    worker.sweep(&mut String::new()).await.unwrap();
    assert_eq!(inbox.submissions.lock().unwrap().len(), 2);
    let good = rows.find_one(filter.clone()).await.unwrap().unwrap();
    assert_eq!(
        good.get_document("delivery")
            .unwrap()
            .get_str("status")
            .unwrap(),
        "accepted"
    );
    *inbox.cursor.lock().unwrap() = Some(observed_cursor(&stream, 0, None));
    rows.update_one(filter.clone(), due).await.unwrap();
    worker.sweep(&mut String::new()).await.unwrap();
    let held = rows.find_one(filter).await.unwrap().unwrap();
    assert_eq!(
        held.get_document("delivery")
            .unwrap()
            .get_str("status")
            .unwrap(),
        "held"
    );
    assert_eq!(
        held.get_document("delivery").unwrap().get("receipt"),
        good.get_document("delivery").unwrap().get("receipt")
    );
    assert_eq!(held.get("payload"), last.get("payload"));
    worker.sweep(&mut String::new()).await.unwrap();
    assert_eq!(inbox.submissions.lock().unwrap().len(), 2);
    f.db.drop().await.unwrap();
}

fn observed_cursor(stream: &str, sequence: u64, digest: Option<String>) -> Cursor {
    Cursor {
        stream_id: stream.into(),
        contiguous_sequence: sequence.to_string(),
        validated_sequence: sequence.to_string(),
        highest_sequence: "2".into(),
        head_digest: digest.clone(),
        validated_head_digest: digest,
        missing_ranges: if sequence == 0 {
            vec![MissingRange {
                from: "1".into(),
                through: "1".into(),
            }]
        } else {
            vec![]
        },
    }
}
