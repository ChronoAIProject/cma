use super::*;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use layer2_core::sandbox::store::SandboxStoreError;
use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use tokio::sync::Notify;

struct Fixture {
    database: mongodb::Database,
    first: Arc<MongoSandboxStore>,
    second: Arc<MongoSandboxStore>,
}
impl Fixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        // Separate clients, pools and service-local state; only Mongo is shared.
        let first = mongodb::Client::with_uri_str(&uri).await.unwrap();
        let second = mongodb::Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_billing_replicas_{}", uuid::Uuid::new_v4().simple());
        let database = first.database(&name);
        let fixture = Self {
            first: Arc::new(MongoSandboxStore::open(&first, &name).await.unwrap()),
            second: Arc::new(MongoSandboxStore::open(&second, &name).await.unwrap()),
            database,
        };
        let event = event();
        fixture.rows().insert_one(doc! {
            "_id":event.transaction_id(),"document_schema_version":1_i64,
            "sandbox_id":event.sandbox_id.as_str(),"owner":event.owner.as_str(),"revision":2_i64,
            "started_at":event.started_at as i64,"ended_at":event.ended_at as i64,
            "record":mongodb::bson::serialize_to_bson(&event).unwrap(),
            "billing_delivered":false,"billing_retry_at":0_i64,"billing_lease_until":0_i64,
        }).await.unwrap();
        fixture
    }
    fn rows(&self) -> mongodb::Collection<Document> {
        self.database.collection("sandbox_usage_events")
    }
    async fn expire(&self) {
        self.rows()
            .update_one(
                doc! {"_id":event().transaction_id()},
                doc! {"$set":{"billing_lease_until":now()-1}},
            )
            .await
            .unwrap();
    }
    async fn row(&self) -> Document {
        self.rows()
            .find_one(doc! {"_id":event().transaction_id()})
            .await
            .unwrap()
            .unwrap()
    }
}

#[derive(Default)]
struct ProviderState {
    block_verification: AtomicBool,
    reject_verification: AtomicBool,
    reject_duplicates: AtomicBool,
    verified: Notify,
    release_verification: Notify,
    block_post: AtomicBool,
    posted: Notify,
    release_post: Notify,
    attempts: AtomicUsize,
    effects: Mutex<BTreeMap<String, Value>>,
}
struct Provider {
    url: String,
    state: Arc<ProviderState>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Provider {
    async fn new() -> Self {
        let state = Arc::new(ProviderState::default());
        let app = Router::new()
            .route("/api/v1/subscriptions/{id}", get(|State(state): State<Arc<ProviderState>>| async move {
                state.verified.notify_one();
                if state.block_verification.swap(false, Ordering::SeqCst) {
                    state.release_verification.notified().await;
                }
                if state.reject_verification.swap(false, Ordering::SeqCst) {
                    return Json(json!({"subscription":null}));
                }
                Json(json!({"subscription":{"external_id":"owner-fixture:starter","external_customer_id":"owner-fixture",
                    "plan_code":"starter","status":"active","billing_time":"calendar","plan":{"charges":payload::METRICS.map(|code|json!({"billable_metric_code":code,"charge_model":"standard"}))}}}))
            }))
            .route("/api/v1/customers/{id}", get(|| async {
                Json(json!({"customer":{"external_id":"owner-fixture","timezone":"UTC"}}))
            }))
            .route("/api/v1/billable_metrics/{code}", get(|axum::extract::Path(code): axum::extract::Path<String>| async move {
                Json(json!({"billable_metric":{"code":code,"aggregation_type":"sum_agg","field_name":"quantity"}}))
            }))
            .route("/api/v1/events/batch", post(|State(state): State<Arc<ProviderState>>, Json(body): Json<Value>| async move {
                // This fixture models a provider with durable transaction-ID
                // idempotency. It is not evidence about an arbitrary endpoint.
                {
                    let mut effects = state.effects.lock().unwrap();
                    if state.reject_duplicates.load(Ordering::SeqCst)
                        && body["events"].as_array().unwrap().iter().any(|event| {
                            effects.contains_key(event["transaction_id"].as_str().unwrap())
                        })
                    {
                        return axum::http::StatusCode::UNPROCESSABLE_ENTITY;
                    }
                    for event in body["events"].as_array().unwrap() {
                        let id = event["transaction_id"].as_str().unwrap().to_owned();
                        if let Some(previous) = effects.get(&id) {
                            assert_eq!(previous, event, "replay must preserve exact provider input");
                        } else {
                            effects.insert(id, event.clone());
                        }
                    }
                }
                state.attempts.fetch_add(1, Ordering::SeqCst);
                state.posted.notify_one();
                if state.block_post.swap(false, Ordering::SeqCst) {
                    state.release_post.notified().await;
                }
                axum::http::StatusCode::CREATED
            })).with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/v1/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { url, state, server }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn independent_clients_exclude_competing_claims_and_refuse_expired_writes() {
    let f = Fixture::new().await;
    let (left, right) = tokio::join!(
        f.first.claim_usage_delivery(now()),
        f.second.claim_usage_delivery(now())
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(
        usize::from(left.is_some()) + usize::from(right.is_some()),
        1
    );
    let original = left.or(right).unwrap();
    assert!(
        f.second
            .claim_usage_delivery(now() + 1000)
            .await
            .unwrap()
            .is_none(),
        "an ahead caller clock cannot steal a server-live claim"
    );
    f.first
        .renew_usage_delivery(&original, now())
        .await
        .unwrap();
    f.expire().await;
    let request =
        json!(payload::freeze(&event(), &config("https://billing.example/api/v1/")).unwrap());
    assert!(matches!(
        f.first
            .freeze_usage_delivery(&original, &request, now() - 300)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert!(matches!(
        f.first.renew_usage_delivery(&original, now() - 300).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert!(
        matches!(
            f.first
                .finish_usage_delivery(&original, now() - 300, None)
                .await,
            Err(SandboxStoreError::Conflict)
        ),
        "clock rollback cannot acknowledge an expired claim"
    );
    let successor = f.second.claim_usage_delivery(now()).await.unwrap().unwrap();
    assert_ne!(original.claim, successor.claim);
    assert!(matches!(
        f.first.finish_usage_delivery(&original, now(), None).await,
        Err(SandboxStoreError::Conflict)
    ));
    f.second
        .freeze_usage_delivery(&successor, &request, now())
        .await
        .unwrap();
    f.second
        .finish_usage_delivery(&successor, now(), None)
        .await
        .unwrap();
    assert!(f
        .first
        .renew_usage_delivery(&successor, now())
        .await
        .is_err());
    assert!(f.row().await.get_bool("billing_delivered").unwrap());
    f.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn ahead_replica_cannot_bypass_provider_retry_pacing() {
    let f = Fixture::new().await;
    let claim = f.first.claim_usage_delivery(now()).await.unwrap().unwrap();
    f.first
        .finish_usage_delivery(&claim, now(), Some("provider_outcome_unknown"))
        .await
        .unwrap();
    assert!(f
        .second
        .claim_usage_delivery(now() + 1000)
        .await
        .unwrap()
        .is_none());
    let row = f.row().await;
    assert!(!row.get_bool("billing_delivered").unwrap());
    assert_eq!(
        row.get_str("billing_error").unwrap(),
        "provider_outcome_unknown"
    );
    assert!(row.get_i64("billing_retry_at").unwrap() > now());
    f.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn later_attempt_failure_cannot_erase_an_unknown_provider_outcome() {
    let f = Fixture::new().await;
    let provider = Provider::new().await;
    provider.state.block_post.store(true, Ordering::SeqCst);
    let first = f.first.claim_usage_delivery(now()).await.unwrap().unwrap();
    let worker = Exporter::new(f.first.clone(), config(&provider.url)).unwrap();
    let attempt = tokio::spawn(async move { worker.deliver(&first).await });
    tokio::time::timeout(Duration::from_secs(5), provider.state.posted.notified())
        .await
        .unwrap();
    attempt.abort();
    assert!(attempt.await.unwrap_err().is_cancelled());
    assert_eq!(provider.state.effects.lock().unwrap().len(), 6);
    f.expire().await;
    let worker = Exporter::new(f.second.clone(), config(&provider.url)).unwrap();
    provider
        .state
        .reject_verification
        .store(true, Ordering::SeqCst);
    provider
        .state
        .reject_duplicates
        .store(true, Ordering::SeqCst);
    for expected in [
        "subscription_identity_or_status_mismatch",
        "provider_rejected_events",
    ] {
        let successor = f.second.claim_usage_delivery(now()).await.unwrap().unwrap();
        let result = worker.deliver(&successor).await;
        assert_eq!(result, Err(expected));
        f.second
            .finish_usage_delivery(&successor, now(), result.err())
            .await
            .unwrap();
        let row = f.row().await;
        assert!(!row.get_bool("billing_delivered").unwrap());
        assert_eq!(
            row.get_str("billing_error").unwrap(),
            "provider_outcome_unknown"
        );
        // Advance only the test fixture's retry clock, preserving the frozen body.
        f.rows()
            .update_one(
                doc! {"_id":event().transaction_id()},
                doc! {"$set":{"billing_retry_at":0_i64}},
            )
            .await
            .unwrap();
    }
    provider
        .state
        .reject_duplicates
        .store(false, Ordering::SeqCst);
    let successor = f.second.claim_usage_delivery(now()).await.unwrap().unwrap();
    worker.deliver(&successor).await.unwrap();
    f.second
        .finish_usage_delivery(&successor, now(), None)
        .await
        .unwrap();
    let row = f.row().await;
    assert!(row.get_bool("billing_delivered").unwrap());
    assert!(matches!(
        row.get("billing_error"),
        Some(mongodb::bson::Bson::Null)
    ));
    assert_eq!(provider.state.effects.lock().unwrap().len(), 6);
    provider.state.release_post.notify_one();
    f.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn takeover_during_provider_verification_refuses_the_former_workers_post() {
    let f = Fixture::new().await;
    let provider = Provider::new().await;
    provider
        .state
        .block_verification
        .store(true, Ordering::SeqCst);
    let original = Arc::new(f.first.claim_usage_delivery(now()).await.unwrap().unwrap());
    let worker = Exporter::new(f.first.clone(), config(&provider.url)).unwrap();
    let token = original.clone();
    let attempt = tokio::spawn(async move { worker.deliver(&token).await });
    tokio::time::timeout(Duration::from_secs(5), provider.state.verified.notified())
        .await
        .unwrap();
    f.expire().await;
    let successor = f.second.claim_usage_delivery(now()).await.unwrap().unwrap();
    provider.state.release_verification.notify_one();
    assert_eq!(attempt.await.unwrap(), Err("billing_claim_lost"));
    assert_eq!(provider.state.attempts.load(Ordering::SeqCst), 0);
    let worker = Exporter::new(f.second.clone(), config(&provider.url)).unwrap();
    worker.deliver(&successor).await.unwrap();
    f.second
        .finish_usage_delivery(&successor, now(), None)
        .await
        .unwrap();
    assert_eq!(provider.state.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(provider.state.effects.lock().unwrap().len(), 6);
    f.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn lost_owner_and_lost_renewal_preserve_unknown_and_replay_exact_provider_ids() {
    for crash in [true, false] {
        let f = Fixture::new().await;
        let provider = Provider::new().await;
        provider.state.block_post.store(true, Ordering::SeqCst);
        let original = Arc::new(f.first.claim_usage_delivery(now()).await.unwrap().unwrap());
        let worker = Exporter::new(f.first.clone(), config(&provider.url)).unwrap();
        let token = original.clone();
        let attempt = tokio::spawn(async move {
            worker
                .deliver_guarded(&token, Duration::from_millis(50))
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), provider.state.posted.notified())
            .await
            .unwrap();
        assert_eq!(
            f.row().await.get_str("billing_error").unwrap(),
            "provider_outcome_unknown"
        );
        if crash {
            attempt.abort();
        }
        f.expire().await;
        let successor = f.second.claim_usage_delivery(now()).await.unwrap().unwrap();
        if crash {
            assert!(attempt.await.unwrap_err().is_cancelled());
        } else {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), attempt)
                    .await
                    .unwrap()
                    .unwrap(),
                Err("provider_outcome_unknown")
            );
        }
        assert!(successor.payload.is_some());
        assert!(f
            .first
            .finish_usage_delivery(&original, now(), None)
            .await
            .is_err());
        let worker = Exporter::new(f.second.clone(), config(&provider.url)).unwrap();
        worker.deliver(&successor).await.unwrap();
        f.second
            .finish_usage_delivery(&successor, now(), None)
            .await
            .unwrap();
        provider.state.release_post.notify_one();
        assert_eq!(provider.state.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            provider.state.effects.lock().unwrap().len(),
            6,
            "two accepted attempts retain exactly six unique metric charges"
        );
        assert!(f.row().await.get_bool("billing_delivered").unwrap());
        f.database.drop().await.unwrap();
    }
}
