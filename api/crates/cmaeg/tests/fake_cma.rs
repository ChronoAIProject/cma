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
}

#[derive(Clone)]
struct StateData {
    records: Arc<Mutex<BTreeMap<String, Stored>>>,
    pub fail_put: Arc<AtomicBool>,
    pub get_delay_ms: Arc<AtomicU64>,
    pub gets: Arc<AtomicU64>,
}

pub struct FakeCma {
    pub base_url: String,
    pub fail_put: Arc<AtomicBool>,
    pub get_delay_ms: Arc<AtomicU64>,
    pub gets: Arc<AtomicU64>,
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
            get_delay_ms: Arc::default(),
            gets: Arc::default(),
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
            get_delay_ms: state.get_delay_ms.clone(),
            gets: state.gets.clone(),
            state,
            shutdown: Some(shutdown),
        }
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
    body: Result<Json<PutRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if state.fail_put.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Json(body) = body.unwrap();
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
            },
        );
        return Json(json!({"record_id":record_id,"version":1,"updated_at":"now"})).into_response();
    };
    if body.expected_version != Some(existing.version) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"actual":existing.version})),
        )
            .into_response();
    }
    existing.owner = body.owner;
    existing.blob = blob;
    existing.version += 1;
    Json(json!({"record_id":record_id,"version":existing.version,"updated_at":"now"}))
        .into_response()
}

async fn get_record(State(state): State<StateData>, Path(record_id): Path<String>) -> Response {
    state.gets.fetch_add(1, Ordering::SeqCst);
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
    let records = state.records.lock().unwrap();
    let items: Vec<_> = records.iter().filter(|(id, record)| {
        query.get("owner").is_none_or(|owner| owner == &record.owner)
            && query.get("after").is_none_or(|after| *id > after)
    }).take(query.get("limit").and_then(|value| value.parse().ok()).unwrap_or(100))
        .map(|(id, record)| json!({"record_id": id, "owner": record.owner, "version": record.version})).collect();
    Json(json!({"items": items, "next_after": null})).into_response()
}
