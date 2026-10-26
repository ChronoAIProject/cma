use super::{types::RETENTION_SECONDS, PlanStore};
use crate::ingress::routing_plan::fixture;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicI64, AtomicUsize, Ordering},
        Mutex,
    },
};

#[derive(Clone)]
pub struct Row {
    pub owner: String,
    pub blob: Vec<u8>,
    pub expiry: i64,
    pub version: u64,
}
pub struct Fault {
    pub id: String,
    pub store: bool,
    pub lose_read: bool,
    pub skip: usize,
}
pub struct Memory {
    pub rows: Mutex<BTreeMap<String, Row>>,
    pub channels: Mutex<BTreeMap<String, Row>>,
    pub fault: Mutex<Option<Fault>>,
    pub clock: Arc<AtomicI64>,
    pub unavailable_reads: AtomicUsize,
    pub writes: AtomicUsize,
}
pub struct Server {
    pub memory: Arc<Memory>,
    pub records: crate::records::RecordClient,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    pub async fn start() -> Self {
        let memory = Arc::new(Memory {
            rows: Mutex::new(BTreeMap::new()),
            channels: Mutex::new(BTreeMap::new()),
            fault: Mutex::new(None),
            clock: Arc::new(AtomicI64::new(fixture::NOW)),
            unavailable_reads: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        });
        let app = Router::new()
            .route("/api/v1/gateway/channel-records/{id}", get(read_channel))
            .route("/api/v1/gateway/interaction-records", get(list))
            .route(
                "/api/v1/gateway/interaction-records/{id}",
                get(read).put(write),
            )
            .with_state(memory.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let records = crate::records::RecordClient::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "record-fixture",
        )
        .unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            memory,
            records,
            task,
        }
    }
    pub fn store(&self) -> PlanStore {
        let mut store = PlanStore::new(self.records.clone(), Arc::new(fixture::keys()));
        let clock = self.memory.clock.clone();
        store.clock = Arc::new(move || clock.load(Ordering::SeqCst));
        store
    }
    pub fn fault(&self, id: String, store: bool, lose_read: bool) {
        self.fault_after(id, store, lose_read, 0);
    }
    pub fn fault_after(&self, id: String, store: bool, lose_read: bool, skip: usize) {
        *self.memory.fault.lock().unwrap() = Some(Fault {
            id,
            store,
            lose_read,
            skip,
        });
    }
    pub fn change(&self, id: &str, mutate: impl FnOnce(&mut serde_json::Value)) {
        let mut rows = self.memory.rows.lock().unwrap();
        let row = rows.get_mut(id).unwrap();
        let opened = cmaeg_core::seal::open_interaction(&fixture::keys(), id, &row.blob).unwrap();
        let mut value = serde_json::from_slice(&opened.payload).unwrap();
        mutate(&mut value);
        row.blob = cmaeg_core::seal::seal_interaction(
            &fixture::keys(),
            id,
            &serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
}
async fn read_channel(State(state): State<Arc<Memory>>, Path(id): Path<String>) -> Response {
    match state.channels.lock().unwrap().get(&id) {
        Some(row) => {
            Json(json!({"version":row.version,"blob":STANDARD.encode(&row.blob)})).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
#[derive(Deserialize)]
struct Put {
    owner: String,
    blob: String,
    expected_version: Option<u64>,
    expires_at: i64,
}
async fn write(
    State(state): State<Arc<Memory>>,
    Path(id): Path<String>,
    Json(input): Json<Put>,
) -> Response {
    let now = state.clock.load(Ordering::SeqCst);
    let Ok(blob) = STANDARD.decode(input.blob) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if blob.len() > 65536 || input.expires_at <= now || input.expires_at > now + RETENTION_SECONDS {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let fault = {
        let mut faults = state.fault.lock().unwrap();
        if faults
            .as_ref()
            .is_some_and(|fault| fault.id == id && fault.skip > 0)
        {
            faults.as_mut().unwrap().skip -= 1;
            None
        } else if faults.as_ref().is_some_and(|fault| fault.id == id) {
            faults.take()
        } else {
            None
        }
    };
    if fault.as_ref().is_some_and(|fault| !fault.store) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let version = {
        let mut rows = state.rows.lock().unwrap();
        let actual = rows.get(&id).map(|row| row.version);
        if actual != input.expected_version {
            return (
                StatusCode::CONFLICT,
                Json(json!({"actual":actual.unwrap_or(0)})),
            )
                .into_response();
        }
        let version = actual.unwrap_or(0) + 1;
        rows.insert(
            id,
            Row {
                owner: input.owner,
                blob,
                expiry: input.expires_at,
                version,
            },
        );
        version
    };
    state.writes.fetch_add(1, Ordering::SeqCst);
    if let Some(fault) = fault {
        if fault.lose_read {
            state.unavailable_reads.store(1, Ordering::SeqCst);
        }
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    } else {
        Json(json!({"version":version})).into_response()
    }
}
async fn read(State(state): State<Arc<Memory>>, Path(id): Path<String>) -> Response {
    if state
        .unavailable_reads
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1))
        .is_ok()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let rows = state.rows.lock().unwrap();
    match rows
        .get(&id)
        .filter(|row| row.expiry > state.clock.load(Ordering::SeqCst))
    {
        Some(row) => {
            Json(json!({"version":row.version,"blob":STANDARD.encode(&row.blob)})).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
#[derive(Deserialize)]
struct List {
    owner: Option<String>,
    after: Option<String>,
    limit: usize,
}
async fn list(State(state): State<Arc<Memory>>, Query(query): Query<List>) -> Response {
    let now = state.clock.load(Ordering::SeqCst);
    let rows = state.rows.lock().unwrap();
    let items = rows
        .iter()
        .filter(|(id, row)| {
            row.expiry > now
                && query.owner.as_ref().is_none_or(|owner| *owner == row.owner)
                && query.after.as_ref().is_none_or(|after| *id > after)
        })
        .take(query.limit)
        .map(|(id, row)| json!({"record_id":id,"owner":row.owner,"version":row.version}))
        .collect::<Vec<_>>();
    Json(json!({"items":items})).into_response()
}
