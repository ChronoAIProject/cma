use super::signer::Signer;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    activity::{ActivityStore, ObservedActivity},
    channels::resolver::ChannelResolver,
    http::callbacks,
    ingress::{
        launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
        IngressState,
    },
    records::RecordClient,
    ChannelCacheConfig, IngressConfig,
};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::ChannelId,
    seal::{seal_blob, ChannelPointer, MasterKeyRing},
};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::task::JoinHandle;

#[path = "activity_records.rs"]
mod activity_records;

pub const ISSUER: &str = "https://nyx.example";
#[derive(Default)]
pub struct Recorder {
    pub requests: Mutex<Vec<LaunchRequest>>,
    pub outcome: AtomicUsize,
    pub wait: tokio::sync::Notify,
    /// When set, each launch first asserts the request body was already released.
    pub released: Mutex<Option<Arc<AtomicBool>>>,
    /// Launches that observed a released request body.
    pub probed: AtomicUsize,
}
impl Recorder {
    /// Accept every launch, asserting first that `dropped` reports the body gone.
    pub fn expect_released(&self, dropped: Arc<AtomicBool>) {
        *self.released.lock().unwrap() = Some(dropped);
        self.outcome.store(1, Ordering::SeqCst);
    }
}
impl TurnLauncher for Recorder {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        if let Some(dropped) = self.released.lock().unwrap().as_ref() {
            assert!(
                dropped.load(Ordering::SeqCst),
                "the request body is still retained while the handler waits for acceptance"
            );
            self.probed.fetch_add(1, Ordering::SeqCst);
        }
        self.requests.lock().unwrap().push(request);
        Box::pin(async {
            match self.outcome.load(Ordering::SeqCst) {
                1 => LaunchOutcome::Accepted {
                    response_id: "response-1".into(),
                },
                2 => {
                    self.wait.notified().await;
                    LaunchOutcome::Deferred
                }
                3 => LaunchOutcome::Refused(cmaeg_core::problem::Problem::new(
                    cmaeg_core::problem::ProblemCode::ProviderUnavailable,
                    503,
                    "provider offline",
                )),
                _ => LaunchOutcome::Deferred,
            }
        })
    }
}
#[derive(Clone)]
pub struct Backend {
    pub interaction_write_status: Arc<AtomicUsize>,
    pub interaction_lose_response: Arc<AtomicBool>,
    pub jwks: Arc<Mutex<Value>>,
    pub gets: Arc<AtomicUsize>,
    pub records: Arc<AtomicUsize>,
    pub record_status: Arc<AtomicUsize>,
    pub status: Arc<AtomicUsize>,
    pub delay_ms: Arc<AtomicUsize>,
    pub blob: Arc<Mutex<Vec<u8>>>,
}
async fn jwks(State(state): State<Backend>) -> Response {
    state.gets.fetch_add(1, Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(
        state.delay_ms.load(Ordering::SeqCst) as u64,
    ))
    .await;
    let status = StatusCode::from_u16(state.status.load(Ordering::SeqCst) as u16).unwrap();
    (
        status,
        [("location", "/trap")],
        Json(state.jwks.lock().unwrap().clone()),
    )
        .into_response()
}
async fn record(State(state): State<Backend>) -> Response {
    state.records.fetch_add(1, Ordering::SeqCst);
    let status = state.record_status.load(Ordering::SeqCst) as u16;
    if status != 200 {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    Json(json!({"blob":STANDARD.encode(state.blob.lock().unwrap().as_slice()),"version":1}))
        .into_response()
}
pub struct Fixture {
    pub platform: String,
    pub state: IngressState,
    pub records: Arc<RecordClient>,
    pub record_origin: String,
    pub activity: Arc<ActivityStore>,
    pub backend: Backend,
    pub recorder: Arc<Recorder>,
    pub id: ChannelId,
    pub url: String,
    pub client: reqwest::Client,
    tasks: Vec<JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl Fixture {
    pub async fn new(edit: impl FnOnce(&mut Value), config: IngressConfig) -> Self {
        Self::on("telegram", edit, config).await
    }
    /// A Telegram relay channel served with `CMAEG_MEDIA_ENABLED=true`.
    pub async fn with_media(edit: impl FnOnce(&mut Value), config: IngressConfig) -> Self {
        Self::build("telegram", edit, config, true, |state| state).await
    }
    /// A channel whose single relay source serves `platform` (a protocol value).
    pub async fn on(platform: &str, edit: impl FnOnce(&mut Value), config: IngressConfig) -> Self {
        Self::build(platform, edit, config, false, |state| state).await
    }
    /// [`Fixture::on`] served with `CMAEG_MEDIA_ENABLED=true`.
    pub async fn on_with_media(
        platform: &str,
        edit: impl FnOnce(&mut Value),
        config: IngressConfig,
    ) -> Self {
        Self::build(platform, edit, config, true, |state| state).await
    }

    pub async fn with_ingress(
        edit: impl FnOnce(&mut Value),
        configure: impl FnOnce(IngressState) -> IngressState,
    ) -> Self {
        Self::build("telegram", edit, IngressConfig::default(), false, configure).await
    }
    async fn build(
        platform: &str,
        edit: impl FnOnce(&mut Value),
        config: IngressConfig,
        media_enabled: bool,
        configure: impl FnOnce(IngressState) -> IngressState,
    ) -> Self {
        let signer = Signer::shared();
        let backend = Backend {
            interaction_write_status: Arc::new(200.into()),
            interaction_lose_response: Arc::new(false.into()),
            jwks: Arc::new(Mutex::new(signer.jwks())),
            gets: Arc::new(0.into()),
            records: Arc::new(0.into()),
            record_status: Arc::new(200.into()),
            status: Arc::new(200.into()),
            delay_ms: Arc::new(0.into()),
            blob: Arc::new(Mutex::new(Vec::new())),
        };
        let (base, server) = serve(
            Router::new()
                .route("/jwks", get(jwks))
                .route("/trap", get(jwks))
                .route("/api/v1/gateway/channel-records/{id}", get(record))
                .with_state(backend.clone())
                .merge(activity_records::router(
                    backend.interaction_write_status.clone(),
                    backend.interaction_lose_response.clone(),
                )),
        )
        .await;
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1; 32]))).unwrap());
        let records = Arc::new(RecordClient::new(&base, "test-service-bearer").unwrap());
        let mut value: Value = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        value["record_id"] = uuid::Uuid::new_v4().to_string().into();
        value["sources"] = json!([{"type":"nyxid_relay","issuer":ISSUER,"jwks_url":format!("{base}/jwks"),"key_id":"key-1","route_ids":["route-1"],"platform":platform,"admission":{"type":"all"}}]);
        if platform == "telegram" {
            value["sources"][0]["bot_username"] = "support_bot".into();
        }
        value["partition"]["key"] = "conversation_and_sender".into();
        edit(&mut value);
        let mut definition: ChannelDefinition = serde_json::from_value(value).unwrap();
        definition.profile_digest = definition.profile.digest();
        definition.validate().unwrap();
        let pointer = ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: records
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
        };
        let id = pointer.seal_id(&keys).unwrap();
        *backend.blob.lock().unwrap() =
            seal_blob(&keys, definition.to_blob_json().to_string().as_bytes()).unwrap();
        let resolver =
            ChannelResolver::new(records.clone(), keys.clone(), ChannelCacheConfig::default());
        let recorder = Arc::new(Recorder::default());
        let state = configure(
            IngressState::new(resolver, keys, config, recorder.clone())
                .unwrap()
                .with_media_enabled(media_enabled),
        );
        let store = Arc::new(ActivityStore::new(
            (*records).clone(),
            state.master_keys.clone(),
        ));
        let observer = Arc::new(ObservedActivity::new(store.clone(), state.activity.clone()));
        let state = state.with_activity_observer(observer);
        let (gateway, app) = serve(callbacks::router(state.clone())).await;
        Self {
            platform: platform.to_owned(),
            url: format!("{gateway}/v1/callbacks/{id}"),
            state,
            records,
            record_origin: base,
            activity: store,
            backend,
            recorder,
            id,
            client: reqwest::Client::new(),
            tasks: vec![server, app],
        }
    }
    pub fn request(&self, body: Vec<u8>, token: &str) -> reqwest::RequestBuilder {
        self.client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("x-nyxid-callback-token", token)
            .header("x-nyxid-platform", &self.platform)
            .header("x-nyxid-message-id", "message-1")
            .header("x-nyxid-timestamp", now_rfc3339())
            .body(body)
    }
    /// The same signed callback as [`Fixture::request`], for in-process handler calls.
    pub fn local_request(&self, body: axum::body::Body, token: &str) -> axum::extract::Request {
        axum::http::Request::post(format!("/v1/callbacks/{}", self.id))
            .header("content-type", "application/json")
            .header("x-nyxid-callback-token", token)
            .header("x-nyxid-platform", &self.platform)
            .header("x-nyxid-message-id", "message-1")
            .header("x-nyxid-timestamp", now_rfc3339())
            .body(body)
            .unwrap()
    }
    pub async fn send(&self, envelope: &Envelope) -> reqwest::Response {
        let (body, token) = envelope.signed();
        self.request(body, &token).send().await.unwrap()
    }
}
pub async fn serve(router: Router) -> (String, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }),
    )
}
pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}
pub fn claims(aud: &str, kind: &str, ttl: i64) -> Value {
    claims_with_clock(aud, kind, ttl, now)
}
pub fn claims_with_clock(aud: &str, kind: &str, ttl: i64, mut clock: impl FnMut() -> i64) -> Value {
    let issued_at = clock();
    json!({"iss":ISSUER,"aud":aud,"token_type":kind,"iat":issued_at,"exp":issued_at+ttl,"jti":"correlation-1"})
}
pub struct Envelope {
    pub payload: Value,
    pub callback: Value,
    pub reply: Value,
}
impl Default for Envelope {
    fn default() -> Self {
        Self::new()
    }
}
impl Envelope {
    pub fn new() -> Self {
        Self::on("telegram")
    }
    /// A callback for `platform`, spelled as NyxID stamps it on every relay artifact.
    pub fn on(platform: &str) -> Self {
        let mut callback = claims("channel-relay/callback", "relay_callback", 300);
        callback["api_key_id"] = "key-1".into();
        callback["message_id"] = "message-1".into();
        callback["platform"] = platform.into();
        let mut reply = claims("channel-relay/reply", "relay_reply", 1800);
        reply["api_key_id"] = "key-1".into();
        reply["inbound_message_id"] = "message-1".into();
        reply["conversation_id"] = "route-1".into();
        reply["platform"] = platform.into();
        let mut payload = json!({"message_id":"message-1","correlation_id":"correlation-1","platform":platform,"agent":{"api_key_id":"key-1","name":"A display name"},"conversation":{"id":"route-1","platform_id":"42","type":"private"},"sender":{"platform_id":"7","display_name":"Alice"},"content":{"type":"text","text":"hello"},"timestamp":now_rfc3339()});
        match platform {
            "telegram" | "telegram-new" => {
                payload["raw_platform_data"] = json!({"message":{"message_id":11,"chat":{"id":42,"type":"private"},"from":{"id":7,"is_bot":false},"text":"hello"}});
            }
            "lark" | "feishu" => {
                payload["conversation"]["platform_id"] = "oc_chat".into();
                payload["sender"]["platform_id"] = "ou_sender".into();
                payload["raw_platform_data"] = json!({"schema":"2.0","header":{"event_id":"lark-event-1","event_type":"im.message.receive_v1","create_time":"1700000000000","app_id":"cli_fixture","tenant_key":"tenant"},"event":{"sender":{"sender_id":{"open_id":"ou_sender","union_id":"on_sender","user_id":"sender"},"sender_type":"user","tenant_key":"tenant"},"message":{"message_id":"om_message","chat_id":"oc_chat","chat_type":"p2p","message_type":"text","content":"{\"text\":\"hello\"}","create_time":"1700000000000"}}});
            }
            _ => {}
        }
        Self {
            callback,
            reply,
            payload,
        }
    }
    pub fn signed(&self) -> (Vec<u8>, String) {
        let mut payload = self.payload.clone();
        if payload.get("reply_token").is_none() {
            payload["reply_token"] = Signer::shared().token(&self.reply).into();
        }
        let body = serde_json::to_vec(&payload).unwrap();
        let mut claims = self.callback.clone();
        claims["body_sha256"] = format!("{:x}", Sha256::digest(&body)).into();
        (body, Signer::shared().token(&claims))
    }
    pub fn group(&mut self) {
        match self.payload["platform"].as_str().unwrap() {
            "telegram" | "telegram-new" => {
                self.payload["raw_platform_data"]["message"]["chat"]["type"] = "supergroup".into();
            }
            "lark" | "feishu" => {
                self.payload["conversation"]["type"] = "group".into();
                self.lark_message()["chat_type"] = "group".into();
            }
            _ => self.payload["conversation"]["type"] = "group".into(),
        }
    }
    /// The raw Lark or Feishu `event.message` object.
    pub fn lark_message(&mut self) -> &mut Value {
        &mut self.payload["raw_platform_data"]["event"]["message"]
    }
    /// Replace the text in both the normalized content and the raw Lark event.
    pub fn lark_text(&mut self, text: &str) {
        self.payload["content"]["text"] = text.into();
        self.lark_message()["content"] = json!({ "text": text }).to_string().into();
    }
}
/// Owns a request body's bytes and records when the last reference to them is dropped.
pub struct DropProbe {
    bytes: Vec<u8>,
    dropped: Arc<AtomicBool>,
}
impl AsRef<[u8]> for DropProbe {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}
/// An in-process request body whose flag turns true once nothing retains its bytes.
///
/// Buffering a single-chunk body shares the probe's allocation instead of
/// copying it, so the flag stays false while any parsed or buffered view lives.
pub fn probed_body(bytes: Vec<u8>) -> (axum::body::Body, Arc<AtomicBool>) {
    let dropped = Arc::new(AtomicBool::new(false));
    let probe = DropProbe {
        bytes,
        dropped: dropped.clone(),
    };
    (
        axum::body::Body::from(bytes::Bytes::from_owner(probe)),
        dropped,
    )
}
pub async fn problem(response: reqwest::Response, status: u16, reason: &str) {
    assert_eq!(response.status().as_u16(), status);
    assert_eq!(response.headers()["cmaeg-protocol-version"], "1");
    assert!(response.headers()["server-timing"]
        .to_str()
        .unwrap()
        .starts_with("restore;dur="));
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("application/problem+json"));
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"], reason, "{body}");
    if status == 401 {
        assert_eq!(body["code"], "callback_invalid");
    }
}

/// Run a telemetry case without other callbacks changing its global counters.
pub fn run_isolated(name: &str) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--ignored"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
