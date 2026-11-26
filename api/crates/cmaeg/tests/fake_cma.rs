use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{net::TcpListener, sync::oneshot};

#[derive(Clone)]
struct Stored {
    owner: String,
    blob: Vec<u8>,
    version: u64,
    expires_at: Option<i64>,
}

#[derive(Clone)]
struct StateData {
    records: Arc<Mutex<BTreeMap<String, Stored>>>,
    pub fail_put: Arc<AtomicBool>,
    put_failures: Arc<AtomicU64>,
    pub get_delay_ms: Arc<AtomicU64>,
    pub gets: Arc<AtomicU64>,
    /// Attempts to mutate a retained record, including refused writes.
    pub writes: Arc<AtomicU64>,
    get_failure_at: Arc<Mutex<Option<u64>>>,
    get_gate: Arc<Mutex<Option<Arc<tokio::sync::Semaphore>>>>,
    put_conflicts: Arc<AtomicU64>,
    lists: Arc<AtomicU64>,
}

pub struct FakeCma {
    pub base_url: String,
    pub fail_put: Arc<AtomicBool>,
    /// Fail only the next N writes, allowing terminal receipt persistence to recover.
    pub put_failures: Arc<AtomicU64>,
    pub get_delay_ms: Arc<AtomicU64>,
    pub gets: Arc<AtomicU64>,
    /// Attempts to mutate a retained record, including refused writes.
    pub writes: Arc<AtomicU64>,
    /// Fail one zero-based record read, independently of provider availability.
    pub get_failure_at: Arc<Mutex<Option<u64>>>,
    /// Optional fixture coordination before the configured read delay starts.
    pub get_gate: Arc<Mutex<Option<Arc<tokio::sync::Semaphore>>>>,
    /// Upcoming writes to an existing record that lose a race: each first
    /// simulates a concurrent writer by bumping the version, then answers 409.
    pub put_conflicts: Arc<AtomicU64>,
    /// Record listing requests served.
    pub lists: Arc<AtomicU64>,
    state: StateData,
    shutdown: Option<oneshot::Sender<()>>,
}

impl FakeCma {
    pub async fn start(fail_put: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = StateData {
            records: Arc::new(Mutex::new(BTreeMap::new())),
            fail_put: Arc::new(AtomicBool::new(fail_put)),
            put_failures: Arc::default(),
            get_delay_ms: Arc::default(),
            gets: Arc::default(),
            writes: Arc::default(),
            get_failure_at: Arc::default(),
            get_gate: Arc::default(),
            put_conflicts: Arc::default(),
            lists: Arc::default(),
        };
        let router = Router::new()
            .route("/api/v1/gateway/channel-records", get(list_records))
            .route(
                "/api/v1/gateway/channel-records/{record_id}",
                put(put_record).get(get_record).delete(delete_record),
            )
            .route(
                "/api/v1/gateway/interaction-records/{record_id}",
                put(put_record).get(get_record).delete(delete_record),
            )
            // Explicit test adapter for the local Mongo WhatsApp retention class.
            // It shares the Interaction rows, but never changes the legacy route.
            .route(
                "/test/whatsapp-reaction-records/{record_id}",
                put(put_record),
            )
            // Test-only transport for the dedicated Mongo-backed persistent domain.
            .route(
                "/api/v1/gateway/thread-fence-records/{record_id}",
                put(put_record).get(get_record),
            )
            .with_state(state.clone())
            .layer(axum::middleware::from_fn(
                |request: axum::extract::Request, next: axum::middleware::Next| async move {
                    if request
                        .headers()
                        .get("authorization")
                        .is_none_or(|value| value != "Bearer cma-service")
                    {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    next.run(request).await
                },
            ));
        let (shutdown, signal) = oneshot::channel();
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = signal.await;
                })
                .await
                .unwrap();
        });
        Self {
            base_url: format!("http://{address}"),
            fail_put: state.fail_put.clone(),
            put_failures: state.put_failures.clone(),
            get_delay_ms: state.get_delay_ms.clone(),
            gets: state.gets.clone(),
            writes: state.writes.clone(),
            get_failure_at: state.get_failure_at.clone(),
            get_gate: state.get_gate.clone(),
            put_conflicts: state.put_conflicts.clone(),
            lists: state.lists.clone(),
            state,
            shutdown: Some(shutdown),
        }
    }

    /// Store a record directly at version 1, as an earlier writer left it.
    pub fn seed(&self, record_id: &str, owner: &str, blob: Vec<u8>) {
        self.state.records.lock().unwrap().insert(
            record_id.to_owned(),
            Stored {
                owner: owner.to_owned(),
                blob,
                version: 1,
                expires_at: None,
            },
        );
    }

    /// Model store TTL removal without advancing the application's wall clock.
    #[allow(dead_code)]
    pub fn expire_at(&self, now: i64) {
        self.state
            .records
            .lock()
            .unwrap()
            .retain(|_, row| row.expires_at.is_none_or(|expiry| expiry > now));
    }

    pub fn records(&self) -> BTreeMap<String, StoredView> {
        self.state
            .records
            .lock()
            .unwrap()
            .iter()
            .map(|(id, record)| {
                (
                    id.clone(),
                    StoredView {
                        owner: record.owner.clone(),
                        blob: record.blob.clone(),
                        version: record.version,
                    },
                )
            })
            .collect()
    }

    pub async fn close(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

#[derive(Clone)]
pub struct StoredView {
    pub owner: String,
    pub blob: Vec<u8>,
    pub version: u64,
}

#[derive(Debug, Deserialize)]
struct PutRequest {
    owner: String,
    blob: String,
    expected_version: Option<u64>,
    expires_at: Option<i64>,
}

#[derive(Debug, Serialize)]
struct RecordResponse {
    record_id: String,
    owner: String,
    blob: String,
    version: u64,
    created_at: String,
    updated_at: String,
}

async fn put_record(
    State(state): State<StateData>,
    Path(record_id): Path<String>,
    uri: axum::http::Uri,
    body: Result<Json<PutRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    state.writes.fetch_add(1, Ordering::SeqCst);
    if state.fail_put.load(Ordering::SeqCst)
        || state
            .put_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Json(body) = body.unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let interaction = uri.path().contains("/interaction-records/");
    let whatsapp_reaction = uri.path().starts_with("/test/whatsapp-reaction-records/");
    if if interaction || whatsapp_reaction {
        body.expires_at.is_none_or(|expiry| {
            expiry <= now
                || expiry
                    > now
                        + if whatsapp_reaction {
                            30 * 86400
                        } else {
                            7 * 86400
                        }
        })
    } else {
        body.expires_at.is_some()
    } {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let blob = STANDARD.decode(body.blob).unwrap();
    let mut records = state.records.lock().unwrap();
    let Some(existing) = records.get_mut(&record_id) else {
        if body.expected_version.is_some() {
            return (StatusCode::CONFLICT, Json(json!({"actual":0}))).into_response();
        }
        records.insert(
            record_id.clone(),
            Stored {
                owner: body.owner.clone(),
                blob,
                version: 1,
                expires_at: body.expires_at,
            },
        );
        return Json(json!({"record_id":record_id,"version":1,"updated_at":"now"})).into_response();
    };
    if state
        .put_conflicts
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
            left.checked_sub(1)
        })
        .is_ok()
    {
        existing.version += 1;
        return (
            StatusCode::CONFLICT,
            Json(json!({"actual":existing.version})),
        )
            .into_response();
    }
    if body.expected_version != Some(existing.version) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"actual":existing.version})),
        )
            .into_response();
    }
    existing.owner = body.owner;
    existing.blob = blob;
    existing.expires_at = body.expires_at;
    existing.version += 1;
    Json(json!({"record_id":record_id,"version":existing.version,"updated_at":"now"}))
        .into_response()
}

async fn get_record(State(state): State<StateData>, Path(record_id): Path<String>) -> Response {
    let attempt = state.gets.fetch_add(1, Ordering::SeqCst);
    if state
        .get_failure_at
        .lock()
        .unwrap()
        .take_if(|at| *at == attempt)
        .is_some()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let gate = state.get_gate.lock().unwrap().clone();
    if let Some(gate) = gate {
        gate.acquire().await.unwrap().forget();
    }
    tokio::time::sleep(std::time::Duration::from_millis(
        state.get_delay_ms.load(Ordering::SeqCst),
    ))
    .await;
    let records = state.records.lock().unwrap();
    let Some(record) = records.get(&record_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(RecordResponse {
        record_id,
        owner: record.owner.clone(),
        blob: STANDARD.encode(&record.blob),
        version: record.version,
        created_at: "now".into(),
        updated_at: "now".into(),
    })
    .into_response()
}

async fn delete_record(
    State(state): State<StateData>,
    Path(record_id): Path<String>,
    query: axum::extract::Query<BTreeMap<String, u64>>,
) -> Response {
    state.writes.fetch_add(1, Ordering::SeqCst);
    let expected = query.0.get("expected_version").copied().unwrap_or_default();
    let mut records = state.records.lock().unwrap();
    let Some(record) = records.get(&record_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if record.version != expected {
        return (StatusCode::CONFLICT, Json(json!({"actual":record.version}))).into_response();
    }
    records.remove(&record_id);
    StatusCode::NO_CONTENT.into_response()
}

async fn list_records(
    State(state): State<StateData>,
    query: axum::extract::Query<BTreeMap<String, String>>,
) -> Response {
    state.lists.fetch_add(1, Ordering::SeqCst);
    let records = state.records.lock().unwrap();
    let items: Vec<_> = records.iter().filter(|(id, record)| {
        query.get("owner").is_none_or(|owner| owner == &record.owner)
            && query.get("after").is_none_or(|after| *id > after)
    }).take(query.get("limit").and_then(|value| value.parse().ok()).unwrap_or(100))
        .map(|(id, record)| json!({"record_id": id, "owner": record.owner, "version": record.version})).collect();
    Json(json!({"items": items, "next_after": null})).into_response()
}
