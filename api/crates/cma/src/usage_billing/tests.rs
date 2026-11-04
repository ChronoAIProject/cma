use super::*;
use layer2_core::{
    sandbox_usage::{Allocation, UsageEvent, UsageEventKind, UsageTotals},
    SandboxId, SubjectId,
};
use mongodb::bson::{doc, Document};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

fn event() -> UsageEvent {
    let allocation = Allocation {
        running: true,
        cpu_millis: 1500,
        memory_mib: 2048,
        disk_gib: 10,
    };
    UsageEvent {
        sandbox_id: SandboxId("sandbox-fixture".into()),
        owner: SubjectId("owner-fixture".into()),
        revision: 2,
        source_seq: 4,
        kind: UsageEventKind::Checkpoint,
        started_at: 86_390,
        ended_at: 86_410,
        quantity: allocation.usage(20).unwrap(),
        next_allocation: allocation.clone(),
        allocation,
        status: "active".into(),
    }
}
fn config(api_url: &str) -> UsageBillingConfig {
    UsageBillingConfig {
        api_url: api_url.parse().unwrap(),
        api_key: "fixture-only".to_string().into(),
        plan_code: "starter".into(),
    }
}

#[test]
fn provider_payload_splits_midnight_and_preserves_exact_integer_quantities() {
    let event = event();
    let batch = payload::freeze(&event, &config("https://billing.example/api/v1/")).unwrap();
    assert_eq!(batch.external_subscription_id, "owner-fixture:starter");
    assert_eq!(batch.events.len(), 6);
    assert_eq!(batch.events[0]["timestamp"], 86_399);
    assert_eq!(batch.events[3]["timestamp"], 86_409);
    assert_ne!(
        batch.events[0]["transaction_id"],
        batch.events[3]["transaction_id"]
    );
    assert_eq!(batch.events[0]["properties"]["quantity"], "15000");
    assert_eq!(batch.events[1]["properties"]["quantity"], "20480");
    assert_eq!(batch.events[2]["properties"]["quantity"], "100");
    let mut corrupted = event.clone();
    corrupted.quantity.cpu_millis_seconds += 1;
    assert_eq!(
        payload::freeze(&corrupted, &config("https://billing.example/api/v1/")).unwrap_err(),
        "usage_quantity_mismatch"
    );
    let mut idle = event;
    idle.allocation = Allocation::default();
    idle.quantity = UsageTotals::default();
    assert!(
        payload::freeze(&idle, &config("https://billing.example/api/v1/"))
            .unwrap()
            .events
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn delivery_retries_a_frozen_provider_request_after_ambiguous_acceptance_and_restart() {
    use axum::{
        routing::{get, post},
        Json, Router,
    };
    let received = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests = Arc::new(AtomicUsize::new(0));
    let captured = received.clone();
    let attempts = requests.clone();
    let app = Router::new()
        .route("/api/v1/billable_metrics/{code}", get(|axum::extract::Path(code): axum::extract::Path<String>| async move {
            Json(json!({ "billable_metric": { "code": code, "aggregation_type": "sum_agg", "field_name": "quantity" } }))
        }))
        .route("/api/v1/customers/{id}", get(|| async { Json(json!({ "customer": { "external_id": "owner-fixture", "timezone": "UTC" } })) }))
        .route("/api/v1/subscriptions/{id}", get(|| async {
            Json(json!({ "subscription": { "external_id": "owner-fixture:starter", "external_customer_id": "owner-fixture",
                "plan_code": "starter", "status": "active", "billing_time": "calendar", "plan": { "charges": payload::METRICS.map(|code| json!({
                    "billable_metric_code": code, "charge_model": "standard", "properties": { "amount": "0" }
                })) } } }))
        }))
        .route("/api/v1/events/batch", post(move |Json(body): Json<Value>| {
            let captured = captured.clone(); let attempts = attempts.clone();
            async move {
                captured.lock().unwrap().push(body);
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 { axum::http::StatusCode::BAD_GATEWAY } else { axum::http::StatusCode::CREATED }
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_url = format!("http://{}/api/v1/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_billing_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let event = event();
    database.collection::<Document>("sandbox_usage_events").insert_one(doc! {
        "_id": event.transaction_id(), "document_schema_version": 1_i64,
        "sandbox_id": event.sandbox_id.as_str(), "owner": event.owner.as_str(), "revision": 2_i64,
        "started_at": event.started_at as i64, "ended_at": event.ended_at as i64,
        "record": mongodb::bson::serialize_to_bson(&event).unwrap(),
        "billing_delivered": false, "billing_retry_at": 0_i64, "billing_lease_until": 0_i64,
    }).await.unwrap();
    let exporter = Exporter::new(store.clone(), config(&api_url)).unwrap();
    let first = store.claim_usage_delivery(100).await.unwrap().unwrap();
    assert!(store.claim_usage_delivery(100).await.unwrap().is_none());
    assert_eq!(
        exporter.deliver(&first).await,
        Err("provider_rejected_events")
    );
    // Crash after the provider may have accepted, before any local acknowledgement.
    let restarted = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let second = restarted.claim_usage_delivery(221).await.unwrap().unwrap();
    assert!(second.payload.is_some());
    let mut changed = config(&api_url);
    changed.plan_code = "changed-default".into();
    let exporter = Exporter::new(restarted.clone(), changed).unwrap();
    exporter.deliver(&second).await.unwrap();
    restarted
        .finish_usage_delivery(&second, 222, None)
        .await
        .unwrap();
    assert!(restarted
        .claim_usage_delivery(1000)
        .await
        .unwrap()
        .is_none());
    {
        let requests = received.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0], requests[1]);
    }
    database.drop().await.unwrap();
    server.abort();
}
