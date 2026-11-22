use super::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

struct Readback {
    receipts: HashMap<String, Value>,
    cursor: Value,
    mode: AtomicU8,
    posts: AtomicUsize,
}

async fn exchange(
    State(state): State<Arc<Readback>>,
    method: Method,
    uri: Uri,
) -> (StatusCode, String) {
    if method == Method::POST {
        state.posts.fetch_add(1, Ordering::SeqCst);
        return (StatusCode::INTERNAL_SERVER_ERROR, String::new());
    }
    if let Some(id) = uri.path().strip_prefix("/usage-receipts/") {
        return (StatusCode::OK, state.receipts[id].to_string());
    }
    assert!(uri.path().starts_with("/usage-cursors/"));
    assert!(uri.query().is_none());
    if state.mode.load(Ordering::SeqCst) == 0 {
        return (
            StatusCode::CONFLICT,
            json!({"error":"conflict"}).to_string(),
        );
    }
    let mut cursor = state.cursor.clone();
    if state.mode.load(Ordering::SeqCst) == 2 {
        cursor["contiguous_validated_sequence"] = "1".into();
        cursor["validated_chain_head"] = "f".repeat(64).into();
    }
    (StatusCode::OK, cursor.to_string())
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn changed_cursor_retries_retained_custody_and_wrong_validated_head_still_holds() {
    let f = Fixture::new(true).await;
    let stream = delivery::stream(&f, "erased-owner-with-retained-obligation").await;
    let rows = f.db.collection::<Document>(OUTBOX);
    let original: Vec<Document> = rows
        .find(doc! {})
        .sort(doc! {"sequence":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let receipts = original.iter().map(|row| {
        let event = FrozenUsageEvent::read(row).unwrap();
        let receipt = json!({"receipt_id":uuid::Uuid::new_v4().to_string(),
            "event_id":event.event_id,"stream_id":event.stream_id,"sequence":event.sequence.to_string(),
            "payload_digest":event.digest,"accepted_at_ms":event.produced_at_ms.to_string(),"custody":"persisted"});
        (event.event_id, json!({"receipt":receipt,"processing":{
            "validation_status":"validation_conflict","validation_reason":"synthetic-conflict",
            "owner_status":"owner_erased","financial_status":"nonbillable_shadow"}}))
    }).collect();
    let last = original.last().unwrap();
    let state = Arc::new(Readback {
        receipts,
        posts: AtomicUsize::new(0),
        mode: AtomicU8::new(0),
        cursor: json!({"stream_id":stream,"contiguous_custody_sequence":"2",
            "contiguous_validated_sequence":"2","highest_received_sequence":"2",
            "custody_chain_head":last.get_str("digest").unwrap(),
            "validated_chain_head":last.get_str("digest").unwrap(),"missing_ranges":[],"next_page":null}),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base: reqwest::Url = format!("http://{}/", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let app = Router::new()
        .fallback(any(exchange))
        .with_state(state.clone());
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let worker = UsageExporter::for_test(
        f.capture(),
        "a".repeat(64),
        http_for_test(base.clone(), Arc::new(Token("fixture-test".into()))),
    );
    for _ in 0..original.len() {
        worker.sweep(&mut String::new()).await.unwrap();
    }
    let filter = doc! {"_id":last.get_str("_id").unwrap()};
    let accepted = rows.find_one(filter.clone()).await.unwrap().unwrap();
    let receipt = accepted
        .get_document("delivery")
        .unwrap()
        .get("receipt")
        .unwrap()
        .clone();
    assert_eq!(
        accepted
            .get_document("delivery")
            .unwrap()
            .get_str("status")
            .unwrap(),
        "accepted"
    );
    let legacy = json!({"stream_id":stream,"contiguous_sequence":"2","validated_sequence":"2",
        "highest_sequence":"2","head_digest":last.get_str("digest").unwrap(),
        "missing_ranges":[],"snapshot":"retained-observation","next_cursor":null});
    let legacy = mongodb::bson::serialize_to_bson(&legacy).unwrap();
    rows.update_one(
        filter.clone(),
        doc! {"$set":{"delivery.cursor":legacy.clone(),
        "delivery.due_at":mongodb::bson::DateTime::from_millis(0)}},
    )
    .await
    .unwrap();
    worker.sweep(&mut String::new()).await.unwrap();
    let retry = rows.find_one(filter.clone()).await.unwrap().unwrap();
    let delivery = retry.get_document("delivery").unwrap();
    assert_eq!(delivery.get_str("status").unwrap(), "pending");
    assert_eq!(delivery.get_str("reason").unwrap(), "cursor_changed");
    assert_eq!(delivery.get("receipt"), Some(&receipt));
    assert_eq!(delivery.get("cursor"), Some(&legacy));
    for (mode, expected) in [(1, "accepted"), (2, "held")] {
        state.mode.store(mode, Ordering::SeqCst);
        rows.update_one(
            filter.clone(),
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
        UsageExporter::for_test(
            reopened,
            "a".repeat(64),
            http_for_test(base.clone(), Arc::new(Token("fixture-restarted".into()))),
        )
        .sweep(&mut String::new())
        .await
        .unwrap();
        let mut observed = rows.find_one(filter.clone()).await.unwrap().unwrap();
        let delivery = observed.remove("delivery").unwrap();
        assert_eq!(
            delivery.as_document().unwrap().get_str("status").unwrap(),
            expected
        );
        assert_eq!(
            delivery.as_document().unwrap().get("receipt"),
            Some(&receipt)
        );
        assert_eq!(delivery.as_document().unwrap().get("cursor"), Some(&legacy));
        assert_eq!(&observed, last);
    }
    assert_eq!(state.posts.load(Ordering::SeqCst), 0);
    f.db.drop().await.unwrap();
}
