use super::{fake_cma::FakeCma, relay_support::Envelope, routing_fixture, signer::Signer};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    channels::resolver::{ChannelResolver, ResolvedChannel},
    delivery::{intents::DeliveryCommand, router::ShardedScheduler},
    http,
    ingress::{
        launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
        IngressState,
    },
    records::RecordClient,
    turns::{
        capacity::RoutedCapacity,
        fanout::FanoutLauncher,
        observer::{ActivityObserver, ActivitySummary, RefusalSummary},
        TurnTracker,
    },
    RoutingConfig,
};
use cmaeg_core::{
    channel::ChannelDefinition,
    routing::Trace,
    seal::{seal_blob, ChannelPointer, MasterKeyRing},
};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Default)]
pub struct Recorder {
    pub requests: Mutex<Vec<LaunchRequest>>,
    pub outcomes: Mutex<BTreeMap<String, LaunchOutcome>>,
}
impl TurnLauncher for Recorder {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        let id = request
            .consumer
            .as_ref()
            .map_or("default", |id| id.as_str());
        let outcome =
            self.outcomes
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .unwrap_or(LaunchOutcome::Accepted {
                    response_id: format!("response-{id}"),
                });
        // Like the real runner, the acceptance-only fake ends its lease on return.
        request.lease.lock().unwrap().take();
        self.requests.lock().unwrap().push(request);
        Box::pin(async move { outcome })
    }
}
#[derive(Default)]
pub struct Observer {
    pub routed: Mutex<Vec<ActivitySummary>>,
    pub refused: Mutex<Vec<RefusalSummary>>,
}
impl ActivityObserver for Observer {
    fn routed(&self, _: &cmaeg_core::ids::RecordId, summary: &ActivitySummary, trace: &Trace) {
        assert!(!format!("{trace:?}").contains("private-message"));
        self.routed.lock().unwrap().push(summary.clone());
    }
    fn refused(&self, _: &cmaeg_core::ids::RecordId, summary: &RefusalSummary) {
        self.refused.lock().unwrap().push(summary.clone());
    }
}
#[derive(Clone, Default)]
pub struct Platform {
    pub calls: Arc<Mutex<Vec<(String, Value)>>>,
}
async fn send(
    State(platform): State<Platform>,
    Path((_token, method)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let chat = body["chat_id"].as_str().unwrap().parse::<i64>().unwrap();
    platform.calls.lock().unwrap().push((method, body));
    Json(json!({"ok":true,"result":{"message_id":42,"chat":{"id":chat}}}))
}
pub struct App {
    pub id: String,
    pub channel: Arc<ResolvedChannel>,
    pub base: String,
    pub platform_base: String,
    pub platform: Platform,
    pub state: IngressState,
    pub recorder: Arc<Recorder>,
    pub observer: Arc<Observer>,
    pub tracker: Arc<TurnTracker>,
    pub capacity: Arc<RoutedCapacity>,
    pub outcomes: Option<mpsc::Receiver<DeliveryCommand>>,
    pub client: reqwest::Client,
    _cma: FakeCma,
    tasks: Vec<JoinHandle<()>>,
}
impl Drop for App {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl App {
    pub async fn new(enabled: bool, edit: impl FnOnce(&mut Value)) -> Self {
        let cma = FakeCma::start(false).await;
        let platform = Platform::default();
        let router = Router::new()
            .route("/jwks", get(|| async { Json(Signer::shared().jwks()) }))
            .route("/{token}/{method}", post(send))
            .with_state(platform.clone());
        let (platform_base, platform_task) = serve(router).await;
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1; 32]))).unwrap());
        let records = Arc::new(RecordClient::new(&cma.base_url, "cma-service").unwrap());
        let mut raw = routing_fixture::routed();
        raw["record_id"] = uuid::Uuid::new_v4().to_string().into();
        raw["sources"] = json!([
            routing_fixture::telegram(),
            routing_fixture::relay("telegram", "route-1"),
            routing_fixture::relay("lark", routing_fixture::ROUTE)
        ]);
        for source in raw["sources"].as_array_mut().unwrap() {
            if source["type"] == "nyxid_relay" {
                source["jwks_url"] = format!("{platform_base}/jwks").into();
            }
        }
        raw["routing"]["default_consumer"]["name"] = "main".into();
        raw["reply"]["sinks"] = json!([{"type":"source"}]);
        edit(&mut raw);
        let definition: ChannelDefinition = serde_json::from_value(raw).unwrap();
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
        cma.seed(
            definition.record_id.as_str(),
            &definition.owner.subject,
            seal_blob(&keys, definition.to_blob_json().to_string().as_bytes()).unwrap(),
        );
        let resolver = ChannelResolver::new(records, keys.clone(), Default::default());
        let channel = resolver.resolve(id.as_str()).await.unwrap();
        let recorder = Arc::new(Recorder::default());
        let observer = Arc::new(Observer::default());
        let tracker = TurnTracker::new(http::readiness::Readiness::initialized());
        let capacity = Arc::new(RoutedCapacity::new(Default::default()));
        let (delivery, outcomes) = mpsc::channel(1);
        let delivery = Arc::new(ShardedScheduler::new(delivery));
        let launcher = Arc::new(FanoutLauncher::new(
            recorder.clone(),
            keys.clone(),
            delivery.clone(),
            capacity.clone(),
            tracker.clone(),
            Duration::from_secs(3),
            observer.clone(),
        ));
        let state = IngressState::new(resolver, keys, Default::default(), launcher, delivery)
            .unwrap()
            .with_routing(
                RoutingConfig {
                    multi_consumer_enabled: enabled,
                    non_human_providers: ["cma_codex".into()].into(),
                    ..Default::default()
                },
                observer.clone(),
            );
        let (base, task) = serve(
            http::callbacks::router(state.clone()).merge(http::telegram::router(state.clone())),
        )
        .await;
        Self {
            id: id.to_string(),
            channel,
            base,
            platform_base,
            platform,
            state,
            recorder,
            observer,
            tracker,
            capacity,
            outcomes: Some(outcomes),
            client: reqwest::Client::new(),
            _cma: cma,
            tasks: vec![task, platform_task],
        }
    }
    pub async fn relay(&self, envelope: &Envelope) -> reqwest::Response {
        let (body, token) = envelope.signed();
        self.client
            .post(format!("{}/v1/callbacks/{}", self.base, self.id))
            .header("content-type", "application/json")
            .header("x-nyxid-callback-token", token)
            .header(
                "x-nyxid-platform",
                envelope.payload["platform"].as_str().unwrap(),
            )
            .header("x-nyxid-message-id", "message-1")
            .header("x-nyxid-timestamp", super::relay_support::now_rfc3339())
            .body(body)
            .send()
            .await
            .unwrap()
    }
    pub async fn native(&self, raw: &Value) -> reqwest::Response {
        self.client
            .post(format!("{}/v1/telegram/{}", self.base, self.id))
            .header("x-telegram-bot-api-secret-token", "fixture-webhook")
            .json(raw)
            .send()
            .await
            .unwrap()
    }
    pub async fn idle(&self) {
        tokio::time::timeout(Duration::from_secs(4), async {
            while !self.tracker.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
async fn serve(router: Router) -> (String, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }),
    )
}
pub fn envelope(text: &str) -> Envelope {
    let mut envelope = Envelope::new();
    envelope.payload["content"]["text"] = text.into();
    envelope.payload["raw_platform_data"]["message"]["text"] = text.into();
    envelope
}
pub fn native(text: &str) -> Value {
    json!({"update_id":101,"message":{"message_id":11,"date":super::relay_support::now()-1,
        "chat":{"id":42,"type":"private"},"from":{"id":7,"is_bot":false},"text":text}})
}
pub fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}
