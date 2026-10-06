use crate::support::{now, Fixture};
use cmaeg::{
    channels::resolver::{ChannelResolver, ResolvedChannel},
    http,
    ingress::{receipts::PlatformReceiptObserver, IngressState},
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, SourceKind},
    problem::{Problem, ProblemCode},
    seal::ReplyAddress,
};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

pub struct Native {
    pub fixture: Fixture,
    pub base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Native {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Native {
    pub async fn new(observer: Option<Arc<dyn PlatformReceiptObserver>>) -> Self {
        let fixture=Fixture::new(|value|{
            value["sources"]=json!([{"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}}]);
            value["secrets"]["telegram_bot_token"]="77:fixture".into();
            value["secrets"]["provider_credential"]="provider-fixture".into();
            value["secrets"]["telegram_webhook_secret"]="native-webhook-fixture".into();
        },Default::default()).await;
        let mut state = fixture.state.clone();
        state.receipt_observer = observer;
        let (base, task) = serve(state).await;
        Self {
            fixture,
            base,
            task,
        }
    }
    pub async fn with_interactions(&mut self, store: Arc<cmaeg::interactions::InteractionStore>) {
        self.task.abort();
        self.fixture.state.interactions = Some(store);
        let (base, task) = serve(self.fixture.state.clone()).await;
        self.base = base;
        self.task = task;
    }
    pub async fn send(&self, raw: &Value, secret: &str) -> reqwest::Response {
        self.fixture
            .client
            .post(format!("{}/v1/telegram/{}", self.base, self.fixture.id))
            .header("x-telegram-bot-api-secret-token", secret)
            .json(raw)
            .send()
            .await
            .unwrap()
    }
    pub async fn accepted(&self, raw: &Value) -> Value {
        let response = self.send(raw, "native-webhook-fixture").await;
        assert_eq!(response.status(), 202);
        let value = response.json::<Value>().await.unwrap();
        assert_eq!(value["status"], "accepted", "{value}");
        value
    }
}
async fn serve(state: IngressState) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, http::telegram::router(state))
            .await
            .unwrap();
    });
    (base, task)
}
pub fn message() -> Value {
    json!({"update_id":101,"message":{"message_id":5,"date":now()-5,"chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Alice"},"text":"hi"}})
}
pub fn creation() -> Value {
    let mut raw = message();
    raw["message"].as_object_mut().unwrap().remove("text");
    raw["message"]["managed_bot_created"] = json!({"bot":{"id":88,"is_bot":true,"username":"created_bot","first_name":"Created","token":"must-stay-private"}});
    raw
}
#[derive(Default)]
struct Observer {
    calls: AtomicUsize,
    fail: AtomicUsize,
}
impl PlatformReceiptObserver for Observer {
    fn observe<'a>(
        &'a self,
        _channel: &'a ResolvedChannel,
        activity: &'a Activity,
    ) -> BoxFuture<'a, Result<(), Problem>> {
        assert!(
            matches!(&activity.kind,ActivityKind::Event{ce}if ce.r#type=="telegram.managed_bot_created")
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if self.fail.load(Ordering::SeqCst) > 0 {
                Err(Problem::new(
                    ProblemCode::ProviderUnavailable,
                    503,
                    "receipt_store_unavailable",
                ))
            } else {
                Ok(())
            }
        })
    }
}
#[tokio::test]
async fn ordinary_chat_and_slash_text_share_provider_and_survive_replica_restart() {
    let observer = Arc::new(Observer::default());
    let mut app = Native::new(Some(observer.clone())).await;
    let mut raw = message();
    raw["message"]["text"] = "/setup hello weather".into();
    let first = app.accepted(&raw).await;
    let original = {
        let requests = app.fixture.recorder.requests.lock().unwrap();
        let request = &requests[0];
        assert!(
            matches!(&request.activity.kind,ActivityKind::Message{text,..}if text=="/setup hello weather")
        );
        assert!(matches!(&request.activity.source,SourceKind::Telegram{bot_id,..}if bot_id=="77"));
        let event = request
            .event_ref
            .open(&app.fixture.state.master_keys, now())
            .unwrap()
            .payload;
        assert!(event.verifies_activity(&request.activity));
        assert!(
            matches!(event.reply,ReplyAddress::Telegram{bot_id,chat_id,message_id,..}if bot_id=="77"&&chat_id=="42"&&message_id=="5")
        );
        request.activity.clone()
    };
    app.task.abort();
    let record_url = app.fixture.recorder.requests.lock().unwrap()[0]
        .channel
        .pointer
        .record_url
        .clone();
    let record_origin = reqwest::Url::parse(&record_url)
        .unwrap()
        .origin()
        .ascii_serialization();
    let resolver = ChannelResolver::new(
        Arc::new(cmaeg::records::RecordClient::new(&record_origin, "test-service-bearer").unwrap()),
        app.fixture.state.master_keys.clone(),
        Default::default(),
    );
    let state = IngressState::new(
        resolver,
        app.fixture.state.master_keys.clone(),
        Default::default(),
        app.fixture.recorder.clone(),
    )
    .unwrap()
    .with_receipt_observer(observer.clone());
    let (base, task) = serve(state).await;
    app.base = base;
    app.task = task;
    assert_eq!(app.accepted(&raw).await, first);
    assert_eq!(
        app.fixture.recorder.requests.lock().unwrap()[1].activity,
        original
    );
    assert_eq!(observer.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn webhook_secret_is_required_and_checked_before_receipt_or_provider() {
    let observer = Arc::new(Observer::default());
    let app = Native::new(Some(observer.clone())).await;
    assert_eq!(app.send(&creation(), "wrong-fixture").await.status(), 401);
    let response = app
        .fixture
        .client
        .post(format!("{}/v1/telegram/{}", app.base, app.fixture.id))
        .json(&creation())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(observer.calls.load(Ordering::SeqCst), 0);
    assert!(app.fixture.recorder.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn authenticated_creation_receipt_has_safe_projection_and_observer_retry() {
    let observer = Arc::new(Observer::default());
    observer.fail.store(1, Ordering::SeqCst);
    let app = Native::new(Some(observer.clone())).await;
    let raw = creation();
    assert_eq!(app.send(&raw, "native-webhook-fixture").await.status(), 503);
    assert!(app.fixture.recorder.requests.lock().unwrap().is_empty());
    observer.fail.store(0, Ordering::SeqCst);
    app.accepted(&raw).await;
    let requests = app.fixture.recorder.requests.lock().unwrap();
    let activity = &requests[0].activity;
    let ActivityKind::Event { ce } = &activity.kind else {
        panic!("expected creation event")
    };
    assert_eq!(ce.source, "telegram:bot:77");
    assert_eq!(ce.data.as_ref().unwrap()["creator_id"], "42");
    assert_eq!(ce.data.as_ref().unwrap()["bot"]["id"], "88");
    assert!(!serde_json::to_string(activity)
        .unwrap()
        .contains("must-stay-private"));
    let event = requests[0]
        .event_ref
        .open(&app.fixture.state.master_keys, now())
        .unwrap()
        .payload;
    assert!(event.verifies_activity(activity));
    let mut tampered = activity.clone();
    tampered.actor.id = "9".into();
    assert!(!event.verifies_activity(&tampered));
}
#[tokio::test]
async fn forged_mixed_forwarded_group_and_bot_creation_updates_never_launch() {
    let app = Native::new(None).await;
    for (pointer, value) in [
        ("/message/from/is_bot", json!(true)),
        ("/message/chat/type", json!("group")),
        ("/message/forward_origin", json!({"type":"user"})),
        ("/message/text", json!("fake receipt")),
        ("/message/managed_bot_created/bot/is_bot", json!(false)),
        ("/message/managed_bot_created/bot/id", json!(0)),
        ("/message/chat/id", json!(43)),
        (
            "/message/managed_bot_created/bot/username",
            json!("bad!name"),
        ),
        ("/message/date", json!(0)),
    ] {
        let mut raw = creation();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        raw.pointer_mut(parent).unwrap()[key] = value;
        let response = app.send(&raw, "native-webhook-fixture").await;
        let value = response.json::<Value>().await.unwrap();
        assert_ne!(value["status"], "accepted", "{pointer}");
    }
    let mut raw = creation();
    raw["managed_bot"] = json!({"bot":{"id":88},"user":{"id":42}});
    assert_ne!(
        app.send(&raw, "native-webhook-fixture")
            .await
            .json::<Value>()
            .await
            .unwrap()["status"],
        "accepted"
    );
    assert!(app.fixture.recorder.requests.lock().unwrap().is_empty());
}
