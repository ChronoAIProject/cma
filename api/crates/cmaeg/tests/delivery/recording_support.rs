//! A recorder around the real `DeliveryEngine`.
//!
//! Relay and Bot API calls go to one in-process `FakeHttp`, webhook calls to
//! a recording `WebhookTransport`, and Sync frames are read from a registered
//! waiter, so under tokio's paused clock no socket is opened and every offset
//! is exact. Each call, receipt and frame is normalized: values that differ on
//! every run (UUIDs, timestamps, signatures and sealed tokens) are replaced by
//! placeholders, and credentials are recorded only as their token class.
use super::{
    fake_http::{FakeHttp, Sent},
    recording_fixtures::{keys, BOT_TOKEN, CHANNEL_KEY, REPLY_TOKEN},
};
use axum::response::{sse::Sse, IntoResponse};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    delivery::{
        custom::CustomDelivery,
        engine::DeliveryEngine,
        intents::{DeliveryCommand, IntentReceipt, ReplySource},
        relay_client::RelayClient,
        sync::SyncRegistry,
        webhook_client::{TransportError, WebhookClient, WebhookTransport},
    },
    http::readiness::Readiness,
    ingress::launch::LaunchRequest,
    interactions::InteractionStore,
    telegram::TelegramClient,
    turns::{outcomes::TurnOutcome, TurnTracker},
    DeliveryConfig,
};
use cmaeg_core::seal::{EventRefPayload, MasterKeyRing, MessageRefPayload};
use futures_util::{future::BoxFuture, StreamExt};
use reqwest::{header::HeaderMap, Url};
use serde_json::{json, Map, Value};
use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::Instant,
};

/// Records each webhook lookup and POST, answering POSTs with `statuses` in
/// order and then 200.
pub struct Hooks {
    calls: Mutex<Vec<(Instant, Value)>>,
    statuses: Mutex<VecDeque<u16>>,
}
impl Hooks {
    pub fn answering(statuses: impl IntoIterator<Item = u16>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::default(),
            statuses: Mutex::new(statuses.into_iter().collect()),
        })
    }
    fn record(&self, call: Value) {
        self.calls.lock().unwrap().push((Instant::now(), call));
    }
}
impl WebhookTransport for Hooks {
    fn resolve(&self, url: Url) -> BoxFuture<'_, Result<Vec<SocketAddr>, TransportError>> {
        self.record(json!({"transport":"dns","endpoint":url.host_str()}));
        Box::pin(async { Ok(vec!["8.8.8.8:443".parse().unwrap()]) })
    }
    fn post(
        &self,
        url: Url,
        _addresses: Vec<SocketAddr>,
        headers: HeaderMap,
        body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>> {
        let mut recorded = Map::new();
        for (name, value) in &headers {
            let value = match name.as_str() {
                "webhook-timestamp" => "<time>".into(),
                "webhook-signature" => "<sig>".into(),
                _ => normalize_text(value.to_str().unwrap()),
            };
            recorded.insert(name.as_str().into(), value.into());
        }
        let body: Value = serde_json::from_slice(&body).unwrap();
        self.record(
            json!({"transport":"webhook","endpoint":url.as_str(),"headers":recorded,"body":normalize(body)}),
        );
        let status = self.statuses.lock().unwrap().pop_front().unwrap_or(200);
        Box::pin(async move { Ok(status) })
    }
}

/// A running engine whose outbound traffic is recorded. Offsets count from
/// `start`, which sends nothing itself, so the first command is at 0 ms.
pub struct Recording {
    pub keys: Arc<MasterKeyRing>,
    pub http: Arc<FakeHttp>,
    hooks: Arc<Hooks>,
    waiters: Arc<SyncRegistry>,
    pub commands: mpsc::Sender<DeliveryCommand>,
    engine: DeliveryEngine,
    started: Instant,
    /// Whether offsets are recorded; only paused-clock offsets are exact.
    timed: bool,
    receipts: Vec<Value>,
    frames: Arc<Mutex<Vec<Value>>>,
    readers: Vec<JoinHandle<()>>,
}
impl Recording {
    pub fn start(http: FakeHttp, hooks: Arc<Hooks>, config: DeliveryConfig) -> Self {
        Self::start_with(http, hooks, config, None, true)
    }
    pub fn start_with(
        http: FakeHttp,
        hooks: Arc<Hooks>,
        config: DeliveryConfig,
        interactions: Option<Arc<InteractionStore>>,
        timed: bool,
    ) -> Self {
        Self::start_with_relay(http, hooks, config, interactions, timed, |relay| relay)
    }
    pub fn start_with_relay(
        http: FakeHttp,
        hooks: Arc<Hooks>,
        config: DeliveryConfig,
        interactions: Option<Arc<InteractionStore>>,
        timed: bool,
        configure: impl FnOnce(RelayClient) -> RelayClient,
    ) -> Self {
        let keys = keys();
        let http = Arc::new(http);
        let telegram = TelegramClient::production(config.reply_timeout)
            .unwrap()
            .with_transport(http.clone());
        let mut relay = RelayClient::new("https://nyx.example", config.reply_timeout)
            .unwrap()
            .with_transport(http.clone())
            .with_telegram(telegram);
        if let Some(store) = interactions {
            relay = relay.with_interactions(store);
        }
        let relay = configure(relay);
        let waiters = Arc::new(SyncRegistry::default());
        let custom = CustomDelivery {
            public_origin: "https://gateway.example".into(),
            waiters: waiters.clone(),
            webhooks: WebhookClient::with_transport(hooks.clone()),
            activity: Arc::new(cmaeg::turns::observer::NoopObserver),
        };
        let (commands, receiver) = mpsc::channel(256);
        let engine = DeliveryEngine::start(
            receiver,
            keys.clone(),
            relay,
            config,
            TurnTracker::new(Readiness::initialized()),
            custom,
        );
        Self {
            keys,
            http,
            hooks,
            waiters,
            commands,
            engine,
            started: Instant::now(),
            timed,
            receipts: vec![],
            frames: Arc::default(),
            readers: vec![],
        }
    }
    fn offset(&self, at: Instant) -> Option<u64> {
        offset(self.started, self.timed, at)
    }
    /// Sleep until `ms` after the start.
    pub async fn at(&self, ms: u64) {
        tokio::time::sleep_until(self.started + Duration::from_millis(ms)).await;
    }
    pub async fn outcome(&self, turn: &Arc<LaunchRequest>, outcome: TurnOutcome) {
        self.commands
            .send(DeliveryCommand::StreamOutcome {
                turn: turn.clone(),
                outcome,
                answer_response_id: None,
                accepted_deadline: None,
                recovery: None,
            })
            .await
            .unwrap();
    }
    /// Send `message` as a reply to `event` and record its receipt.
    pub async fn reply(
        &mut self,
        channel: &Arc<ResolvedChannel>,
        event: &EventRefPayload,
        message: Value,
        source: ReplySource,
    ) -> IntentReceipt {
        let (receipt, received) = oneshot::channel();
        self.commands
            .send(DeliveryCommand::Reply {
                trace: tracing::Span::none(),
                channel: channel.clone(),
                event: event.clone(),
                message: serde_json::from_value(message).unwrap(),
                source,
                receipt,
            })
            .await
            .unwrap();
        self.receipt("reply", received.await.unwrap())
    }
    /// Replace the message `reference` names and record the receipt.
    pub async fn update(
        &mut self,
        channel: &Arc<ResolvedChannel>,
        reference: MessageRefPayload,
        message: Value,
    ) -> IntentReceipt {
        let (receipt, received) = oneshot::channel();
        self.commands
            .send(DeliveryCommand::Update {
                trace: tracing::Span::none(),
                channel: channel.clone(),
                message_ref: reference,
                message: serde_json::from_value(message).unwrap(),
                receipt,
            })
            .await
            .unwrap();
        self.receipt("update", received.await.unwrap())
    }
    fn receipt(&mut self, command: &str, receipt: IntentReceipt) -> IntentReceipt {
        let mut entry =
            json!({"command":command,"receipt":normalize(serde_json::to_value(&receipt).unwrap())});
        if let Some(at) = self.offset(Instant::now()) {
            entry["at_ms"] = at.into();
        }
        self.receipts.push(entry);
        receipt
    }
    /// Register a Sync waiter whose frames are recorded as they arrive.
    pub fn waiter(&mut self) -> String {
        let (id, waiter) = self.waiters.register().unwrap();
        let (frames, started, timed) = (self.frames.clone(), self.started, self.timed);
        self.readers.push(tokio::spawn(async move {
            let mut body = Sse::new(waiter.stream())
                .into_response()
                .into_body()
                .into_data_stream();
            while let Some(chunk) = body.next().await {
                let text = String::from_utf8(chunk.unwrap().to_vec()).unwrap();
                let field = |prefix: &str| {
                    text.lines()
                        .find_map(|line| line.strip_prefix(prefix))
                        .unwrap()
                        .to_owned()
                };
                let data: Value = serde_json::from_str(&field("data: ")).unwrap();
                let mut frame = json!({"event":field("event: "),"data":normalize(data)});
                if let Some(at) = offset(started, timed, Instant::now()) {
                    frame["at_ms"] = at.into();
                }
                frames.lock().unwrap().push(frame);
            }
        }));
        id
    }
    /// Drain the engine and return the normalized calls, receipts and frames.
    pub async fn finish(self) -> Value {
        let (started, timed) = (self.started, self.timed);
        // A std deadline far beyond any paused-clock offset a recording reaches.
        assert!(
            self.engine
                .drain(std::time::Instant::now() + Duration::from_secs(3600))
                .await
        );
        for reader in self.readers {
            tokio::time::timeout(Duration::from_secs(60), reader)
                .await
                .expect("a Sync waiter never ended")
                .unwrap();
        }
        let mut calls: Vec<(Instant, Value)> = self
            .http
            .sent()
            .iter()
            .map(|sent| (sent.at, http_call(sent)))
            .collect();
        calls.extend(self.hooks.calls.lock().unwrap().iter().cloned());
        calls.sort_by_key(|(at, _)| *at);
        let calls: Vec<Value> = calls
            .into_iter()
            .map(|(at, mut call)| {
                if let Some(at) = offset(started, timed, at) {
                    call["at_ms"] = at.into();
                }
                call
            })
            .collect();
        let frames = self.frames.lock().unwrap().clone();
        json!({"calls":calls,"receipts":self.receipts,"sync":frames})
    }
}

/// Milliseconds from `started` to `at`, when the recording is timed.
fn offset(started: Instant, timed: bool, at: Instant) -> Option<u64> {
    timed.then(|| millis(at - started))
}
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap()
}

/// A relay or Bot API call with its credential reduced to its token class.
fn http_call(sent: &Sent) -> Value {
    let path = sent.url.path();
    let (transport, endpoint, credential) = match path.strip_prefix(&format!("/bot{BOT_TOKEN}/")) {
        Some(method) => ("telegram", method, "bot_token"),
        None => (
            "relay",
            path,
            match sent.header("authorization") {
                Some(value) if value == format!("Bearer {REPLY_TOKEN}") => "reply_token",
                Some(value) if value == format!("Bearer {CHANNEL_KEY}") => "channel_key",
                _ => "unrecognized",
            },
        ),
    };
    json!({"transport":transport,"endpoint":endpoint,"credential":credential,
        "content_type":sent.header("content-type"),"timeout_ms":millis(sent.timeout),
        "body":normalize(sent.json())})
}

/// Replace every per-run value inside `value`.
pub fn normalize(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(normalize_text(&text)),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(name, value)| (name, normalize(value)))
                .collect(),
        ),
        other => other,
    }
}

/// An RFC 3339 timestamp becomes `<time>`, a sealed token `<sealed:kind>`,
/// and each version 4 UUID inside the text `<uuid>`.
pub fn normalize_text(text: &str) -> String {
    let rfc3339 = &time::format_description::well_known::Rfc3339;
    if time::OffsetDateTime::parse(text, rfc3339).is_ok() {
        return "<time>".into();
    }
    let parts: Vec<&str> = text.split('.').collect();
    if let ["cmaeg1", kind, kid, _sealed] = parts.as_slice() {
        if kid.bytes().all(|byte| byte.is_ascii_digit()) {
            return format!("<sealed:{kind}>");
        }
    }
    let mut normalized = String::with_capacity(text.len());
    let mut rest = text;
    let mut bounded = true;
    while let Some(next) = rest.chars().next() {
        if let Some(length) = bounded.then(|| uuid_length(rest)).flatten() {
            normalized.push_str("<uuid>");
            rest = &rest[length..];
            continue;
        }
        normalized.push(next);
        bounded = !next.is_ascii_alphanumeric();
        rest = &rest[next.len_utf8()..];
    }
    normalized
}

/// The length of a hyphenated or simple version 4 UUID that opens `text` and
/// is not followed by another alphanumeric character.
fn uuid_length(text: &str) -> Option<usize> {
    [36, 32].into_iter().find(|&length| {
        text.get(..length)
            .and_then(|candidate| uuid::Uuid::try_parse(candidate).ok())
            .is_some_and(|id| id.get_version_num() == 4)
            && !text[length..].starts_with(|next: char| next.is_ascii_alphanumeric())
    })
}
