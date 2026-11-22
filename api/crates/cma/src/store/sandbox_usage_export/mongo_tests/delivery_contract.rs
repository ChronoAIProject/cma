use super::*;
use crate::{
    producer_token::{ProducerToken, ProducerTokenError, ProducerTokenProvider},
    sandbox_usage_export::{http_for_test, Receipt, UsageExporter},
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    routing::any,
    Router,
};
use futures_util::TryStreamExt;
use std::collections::HashMap;

mod cursor;

struct Inbox {
    expected: HashMap<String, Document>,
    receipts: Collection<Document>,
    submitted: Mutex<Vec<String>>,
}

async fn exchange(
    State(inbox): State<Arc<Inbox>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    bytes: Bytes,
) -> (StatusCode, String) {
    assert!(headers["authorization"]
        .to_str()
        .unwrap()
        .starts_with("Bearer fixture-"));
    if method == Method::POST && uri.path() == "/usage-events" {
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = value["event_id"].as_str().unwrap();
        let row = &inbox.expected[id];
        assert_eq!(&*bytes, row.get_binary_generic("payload").unwrap());
        super::super::fixture_tests::assert_wire_event(&value);
        let event = FrozenUsageEvent::read(row).unwrap();
        let receipt: Receipt = serde_json::from_value(serde_json::json!({
            "receipt_id":uuid::Uuid::new_v4().to_string(), "event_id":id,
            "stream_id":event.stream_id,"sequence":event.sequence.to_string(),
            "payload_digest":event.digest,"accepted_at_ms":event.produced_at_ms.to_string(),
            "custody":"persisted"
        }))
        .unwrap();
        receipt.validate(&event).unwrap();
        inbox
            .receipts
            .insert_one(
                doc! {"_id":id,"receipt":mongodb::bson::serialize_to_bson(&receipt).unwrap()},
            )
            .await
            .unwrap();
        inbox.submitted.lock().unwrap().push(id.into());
        // Custody committed, but the submitter receives no receipt.
        return (StatusCode::SERVICE_UNAVAILABLE, String::new());
    }
    assert_eq!(method, Method::GET);
    if let Some(id) = uri.path().strip_prefix("/usage-receipts/") {
        if let Some(row) = inbox.receipts.find_one(doc! {"_id":id}).await.unwrap() {
            let receipt: Receipt =
                mongodb::bson::deserialize_from_bson(row.get("receipt").unwrap().clone()).unwrap();
            return (
                StatusCode::OK,
                serde_json::json!({
                    "receipt":receipt,"processing":{
                        "validation_status":"waiting_for_predecessor","validation_reason":null,
                        "owner_status":"owner_unresolved","financial_status":"nonbillable_shadow"
                    }
                })
                .to_string(),
            );
        }
    } else {
        assert!(uri.path().starts_with("/usage-cursors/"));
    }
    (StatusCode::NOT_FOUND, String::new())
}

struct Token(String);
#[async_trait::async_trait]
impl ProducerTokenProvider for Token {
    async fn token(&self) -> Result<ProducerToken, ProducerTokenError> {
        Ok(ProducerToken::new(self.0.clone()))
    }
}

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn recover_actual_events(fixture: &Fixture) {
    let outbox = crate::store::durability::majority_collection::<Document>(&fixture.db, OUTBOX);
    let before: Vec<Document> = outbox
        .find(doc! {})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(!before.is_empty());
    let expected: HashMap<_, _> = before
        .into_iter()
        .map(|row| (row.get_str("_id").unwrap().to_owned(), row))
        .collect();
    let inbox = Arc::new(Inbox {
        expected,
        receipts: crate::store::durability::majority_collection(
            &fixture.db,
            "fixture_http_receipts",
        ),
        submitted: Default::default(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base: reqwest::Url = format!("http://{}/", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let app = Router::new()
        .fallback(any(exchange))
        .with_state(inbox.clone());
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let peer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    for iteration in 0..inbox.expected.len() * 2 {
        let capture = CaptureStore::open(&peer, fixture.db.name(), config())
            .await
            .unwrap();
        let transport = http_for_test(
            base.clone(),
            Arc::new(Token(format!("fixture-token-{iteration}"))),
        );
        UsageExporter::for_test(capture, "a".repeat(64), transport)
            .sweep(&mut String::new())
            .await
            .unwrap();
        outbox
            .update_many(
                doc! {"delivery.status":"pending"},
                doc! {"$set":{
                    "delivery.due_at":mongodb::bson::DateTime::from_millis(0)
                }},
            )
            .await
            .unwrap();
    }
    assert_eq!(inbox.submitted.lock().unwrap().len(), inbox.expected.len());
    assert_eq!(
        inbox.receipts.count_documents(doc! {}).await.unwrap(),
        inbox.expected.len() as u64
    );
    let after: Vec<Document> = outbox
        .find(doc! {})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(after.len(), inbox.expected.len());
    for mut row in after {
        let id = row.get_str("_id").unwrap().to_owned();
        let delivery = row.remove("delivery").unwrap();
        assert_eq!(
            delivery.as_document().unwrap().get_str("status").unwrap(),
            "accepted"
        );
        assert_eq!(row, inbox.expected[&id]);
    }
}
