//! Real delivery with only in-process IO, suitable for a paused Tokio clock.
use super::{
    default_fixtures::{default_channel, sinks_default, with_sinks},
    fake_http::FakeHttp,
    group_fixtures::{consumer_event, consumer_turn, reviewer, routed_custom_activity},
    recording_fixtures::keys,
    recording_support::Hooks,
};
use axum::response::{sse::Sse, IntoResponse};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    delivery::{
        custom::CustomDelivery,
        engine::DeliveryEngine,
        intents::{DeliveryCommand, IntentReceipt, ReplySource},
        relay_client::RelayClient,
        router::{DeliverySink, ShardedScheduler},
        sync::SyncRegistry,
        webhook_client::{TransportError, WebhookClient, WebhookTransport},
    },
    http::readiness::Readiness,
    ingress::launch::LaunchRequest,
    telegram::TelegramClient,
    tools::ConsumerSummary,
    turns::{outcomes::TurnOutcome, TurnTracker},
    DeliveryConfig,
};
use cmaeg_core::{
    channel::ReplyChoice,
    ids::ConsumerName,
    seal::{MasterKeyRing, ReplyAddress},
};
use futures_util::{future::BoxFuture, StreamExt};
use reqwest::{header::HeaderMap, Url};
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, Notify},
    task::JoinHandle,
    time::Instant,
};

pub type Frame = (u64, String, Value);
struct SlowHooks {
    hooks: Arc<Hooks>,
    delay: Duration,
    started: Instant,
    entries: Arc<Mutex<Vec<(u64, Value)>>>,
    changed: Arc<Notify>,
}
impl WebhookTransport for SlowHooks {
    fn resolve(&self, url: Url) -> BoxFuture<'_, Result<Vec<SocketAddr>, TransportError>> {
        self.hooks.resolve(url)
    }
    fn post(
        &self,
        url: Url,
        addresses: Vec<SocketAddr>,
        headers: HeaderMap,
        body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>> {
        Box::pin(async move {
            self.entries.lock().unwrap().push((
                self.started.elapsed().as_millis() as u64,
                serde_json::from_slice(&body).unwrap(),
            ));
            self.changed.notify_one();
            tokio::time::sleep(self.delay).await;
            self.hooks.post(url, addresses, headers, body).await
        })
    }
}

pub struct Harness {
    pub keys: Arc<MasterKeyRing>,
    pub channel: Arc<ResolvedChannel>,
    pub waiters: Arc<SyncRegistry>,
    pub tracker: Arc<TurnTracker>,
    pub sink: Arc<ShardedScheduler>,
    pub receiver: Option<mpsc::Receiver<DeliveryCommand>>,
    pub turns: [Arc<LaunchRequest>; 2],
    pub frames: Arc<Mutex<Vec<Frame>>>,
    changed: Arc<Notify>,
    hooks: Arc<Hooks>,
    delay: Duration,
    started: Instant,
    hook_entries: Arc<Mutex<Vec<(u64, Value)>>>,
    reader: JoinHandle<()>,
    engine: Option<DeliveryEngine>,
}
impl Harness {
    pub fn new(multiplex: bool, capacity: usize, delay: Duration) -> Self {
        let channel = default_channel(sinks_default(), with_sinks);
        let keys = keys();
        let waiters = Arc::new(SyncRegistry::default());
        let (id, waiter) = if multiplex {
            let (id, waiter) = waiters
                .register_fanout(
                    2,
                    ConsumerSummary {
                        id: cmaeg_core::ids::ConsumerId::default_consumer(),
                        name: ConsumerName::parse("lead").unwrap(),
                        label: "lead".into(),
                    },
                )
                .unwrap();
            assert!(
                waiters.set_fanout_targets(&id, &routed_custom_activity().routing.unwrap().targets)
            );
            (id, waiter)
        } else {
            waiters.register().unwrap()
        };
        let turns = [None, Some(reviewer())].map(|consumer| {
            let activity = routed_custom_activity();
            let mut event = consumer_event(&channel, &activity, consumer);
            event.reply = ReplyAddress::Sync { waiter: id.clone() };
            let mut turn = consumer_turn(&keys, &channel, activity, &event);
            Arc::get_mut(&mut turn).unwrap().sync_fanout = multiplex;
            turn
        });
        let frames = Arc::new(Mutex::new(vec![]));
        let changed = Arc::new(Notify::new());
        let output = frames.clone();
        let wake = changed.clone();
        let started = Instant::now();
        let reader = tokio::spawn(async move {
            let mut body = Sse::new(waiter.stream())
                .into_response()
                .into_body()
                .into_data_stream();
            while let Some(chunk) = body.next().await {
                let chunk = String::from_utf8(chunk.unwrap().to_vec()).unwrap();
                let field = |prefix| {
                    chunk
                        .lines()
                        .find_map(|line| line.strip_prefix(prefix))
                        .unwrap()
                };
                let frame = (
                    started.elapsed().as_millis() as u64,
                    field("event: ").to_owned(),
                    serde_json::from_str(field("data: ")).unwrap(),
                );
                output.lock().unwrap().push(frame);
                wake.notify_one();
            }
        });
        let (sender, receiver) = mpsc::channel(capacity);
        Self {
            keys,
            channel,
            waiters,
            tracker: TurnTracker::new(Readiness::initialized()),
            sink: Arc::new(ShardedScheduler::new(sender)),
            receiver: Some(receiver),
            turns,
            frames,
            changed,
            hooks: Hooks::answering([]),
            delay,
            started,
            hook_entries: Arc::default(),
            reader,
            engine: None,
        }
    }
    pub fn start(&mut self) {
        let http = Arc::new(FakeHttp::platforms());
        let telegram = TelegramClient::production(Duration::from_secs(2))
            .unwrap()
            .with_transport(http.clone());
        let relay = RelayClient::new("https://nyx.example", Duration::from_secs(2))
            .unwrap()
            .with_transport(http)
            .with_telegram(telegram);
        let custom = CustomDelivery {
            public_origin: "https://gateway.example".into(),
            waiters: self.waiters.clone(),
            webhooks: WebhookClient::with_transport(Arc::new(SlowHooks {
                hooks: self.hooks.clone(),
                delay: self.delay,
                started: self.started,
                entries: self.hook_entries.clone(),
                changed: self.changed.clone(),
            })),
        };
        self.engine = Some(DeliveryEngine::start(
            self.receiver.take().unwrap(),
            self.keys.clone(),
            relay,
            DeliveryConfig {
                multi_consumer_enabled: true,
                ..Default::default()
            },
            self.tracker.clone(),
            custom,
        ));
    }
    pub async fn outcome(&self, turn: &Arc<LaunchRequest>, outcome: TurnOutcome) {
        self.sink
            .submit(DeliveryCommand::StreamOutcome {
                turn: turn.clone(),
                outcome,
            })
            .await
            .unwrap();
    }
    pub async fn reply(
        &self,
        turn: &Arc<LaunchRequest>,
        default: bool,
        text: &str,
    ) -> oneshot::Receiver<IntentReceipt> {
        let event = turn
            .event_ref
            .open(&self.keys, time::OffsetDateTime::now_utc().unix_timestamp())
            .unwrap()
            .payload;
        let (receipt, received) = oneshot::channel();
        self.sink
            .submit(DeliveryCommand::Reply {
                trace: tracing::Span::none(),
                channel: self.channel.clone(),
                event,
                message: serde_json::from_value(json!({"text":text})).unwrap(),
                source: ReplySource::Tool,
                destination: default.then_some(ReplyChoice::Default),
                receipt,
            })
            .await
            .unwrap();
        received
    }
    pub async fn until_hook(&self, text: &str) -> u64 {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some((at, _)) = self
                    .hook_entries
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(_, body)| body["data"]["text"] == text)
                {
                    return *at;
                }
                self.changed.notified().await;
            }
        })
        .await
        .expect("earlier webhook entered its delay")
    }
    pub async fn until(&self, name: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self
                .frames
                .lock()
                .unwrap()
                .iter()
                .any(|frame| frame.1 == name)
            {
                self.changed.notified().await;
            }
        })
        .await
        .expect("expected Sync frame");
    }
    pub async fn finish(self) -> (Vec<Frame>, Vec<Value>) {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        assert!(self.tracker.drain(deadline).await);
        assert!(self.engine.unwrap().drain(deadline).await);
        tokio::time::timeout(Duration::from_secs(5), self.reader)
            .await
            .unwrap()
            .unwrap();
        assert!(self.waiters.is_empty());
        let frames = self.frames.lock().unwrap().clone();
        let bodies = self
            .hooks
            .raw_bodies
            .lock()
            .unwrap()
            .iter()
            .map(|body| serde_json::from_str(body).unwrap())
            .collect();
        (frames, bodies)
    }
}
