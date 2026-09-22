use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{channels::resolver::ChannelResolver, records::RecordClient, ChannelCacheConfig};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::ChannelId,
    seal::{seal_blob, ChannelPointer, MasterKeyRing},
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::oneshot;

type StoredRecord = Option<(Vec<u8>, u64)>;

#[derive(Clone, Default)]
pub struct StateData {
    pub record: Arc<Mutex<StoredRecord>>,
    pub gets: Arc<AtomicUsize>,
    pub delay_ms: Arc<AtomicU64>,
    pub fail: Arc<AtomicBool>,
}
pub struct FakeRecords {
    pub base: String,
    pub state: StateData,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl FakeRecords {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mut server = Self {
            base,
            state: StateData::default(),
            stop: None,
            task: None,
        };
        server.serve(listener);
        server
    }
    fn serve(&mut self, listener: tokio::net::TcpListener) {
        let (stop, signal) = oneshot::channel();
        let router = Router::new()
            .route("/ready", get(|| async { StatusCode::OK }))
            .route("/api/v1/gateway/channel-records/{record_id}", get(record))
            .with_state(self.state.clone());
        self.stop = Some(stop);
        self.task = Some(tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = signal.await;
                })
                .await
                .unwrap();
        }));
    }
    pub async fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await.unwrap();
        }
    }
    pub async fn restart(&mut self) {
        let listener = tokio::net::TcpListener::bind(self.base.trim_start_matches("http://"))
            .await
            .unwrap();
        self.serve(listener);
    }
    pub fn count(&self) -> usize {
        self.state.gets.load(Ordering::SeqCst)
    }
    pub async fn wait_for_gets(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.count() < count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
async fn record(State(state): State<StateData>) -> Response {
    state.gets.fetch_add(1, Ordering::SeqCst);
    let record = state.record.lock().unwrap().clone();
    let fail = state.fail.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(state.delay_ms.load(Ordering::SeqCst))).await;
    if fail {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    match record {
        Some((blob, version)) => {
            Json(serde_json::json!({"blob": STANDARD.encode(blob), "version": version}))
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub struct Fixture {
    pub server: FakeRecords,
    pub resolver: ChannelResolver,
    pub keys: Arc<MasterKeyRing>,
    pub id: ChannelId,
    pub definition: ChannelDefinition,
    pub records: Arc<RecordClient>,
}
impl Fixture {
    pub async fn new(config: ChannelCacheConfig) -> Self {
        let server = FakeRecords::start().await;
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1_u8; 32]))).unwrap());
        let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        definition.profile_digest = definition.profile.digest();
        let records = Arc::new(RecordClient::new(&server.base, "service-fixture").unwrap());
        let pointer = ChannelPointer {
            v: 1,
            record_url: records
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
            record_id: definition.record_id.clone(),
        };
        let id = pointer.seal_id(&keys).unwrap();
        let resolver = ChannelResolver::new(records.clone(), keys.clone(), config);
        let fixture = Self {
            server,
            resolver,
            keys,
            id,
            definition,
            records,
        };
        fixture.store(1);
        fixture
    }
    pub fn store(&self, version: u64) {
        let blob = seal_blob(
            &self.keys,
            self.definition.to_blob_json().to_string().as_bytes(),
        )
        .unwrap();
        *self.server.state.record.lock().unwrap() = Some((blob, version));
    }
}
