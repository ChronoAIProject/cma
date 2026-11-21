//! Shared helpers for the media retrieval tests: loopback servers, request
//! fixtures and log capture under the gateway's own filter.
use axum::Router;
use cmaeg::{
    channels::resolver::ResolvedChannel,
    media::{
        body::Overflow,
        source::{FetchRequest, SourceCredential},
    },
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, AttachmentSource, SourceKind},
    channel::ChannelDefinition,
    ids::ChannelId,
    seal::{ChannelPointer, EventRefPayload, ReplyAddress},
};
use std::{
    future::Future,
    io::{self, Write},
    sync::{Arc, Mutex, Once, OnceLock},
    time::{Duration, Instant as StdInstant},
};
use tokio::{net::TcpListener, time::Instant};
use tracing_subscriber::{layer::SubscriberExt, Layer};

/// Every HTTP client crate at trace, as the leak tests' worst-case operator.
pub const TRACE_ENV: &str = "trace,reqwest::connect=trace,hyper_util::client=trace,h2=trace";

/// Serve `router` on a loopback port and return its origin.
pub async fn serve(router: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{address}")
}

/// Start a fake on its own thread and runtime, so the fake's own tracing
/// never reaches a test's scoped subscriber and only the gateway's is judged.
pub fn isolated<T, F>(start: impl FnOnce() -> F + Send + 'static) -> T
where
    T: Send + 'static,
    F: Future<Output = T>,
{
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            sender.send(start().await).unwrap();
            std::future::pending::<()>().await;
        });
    });
    receiver.recv().unwrap()
}

/// A deadline no well-behaved test reaches.
pub fn far() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

pub fn channel_id() -> ChannelId {
    ChannelId::new("cmaeg1.ch.media-fixture").unwrap()
}

/// The fixture channel after `edit`, as the resolver would return it.
pub fn resolved(edit: impl FnOnce(&mut ChannelDefinition)) -> ResolvedChannel {
    let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    edit(&mut definition);
    ResolvedChannel {
        id: channel_id(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
        },
        definition,
        agent_key_hash: [0; 32],
        consumer_keys: Vec::new(),
        loaded_at: StdInstant::now(),
        version: 1,
    }
}

pub fn event(reply: ReplyAddress) -> EventRefPayload {
    EventRefPayload {
        telegram_admission: None,
        v: 1,
        activity_sha256: None,
        channel_id: channel_id(),
        record_id: "record-1".parse().unwrap(),
        event_id: "msg-1".parse().unwrap(),
        partition_key: "msg-1".into(),
        conversation_id: "conversation-1".parse().unwrap(),
        expires_at: unix_now() + 1_800,
        reply,
        consumer: None,
        lineage: None,
    }
}

/// The fixture activity with `source` and `kind`.
pub fn activity(source: SourceKind, kind: Option<ActivityKind>) -> Activity {
    let mut activity: Activity = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/activity.json"
    ))
    .unwrap();
    activity.source = source;
    if let Some(kind) = kind {
        activity.kind = kind;
    }
    activity
}

pub fn unix_now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// A request reading the whole attachment, up to `cap`, before `deadline`.
pub fn request<'a>(
    channel: &'a ResolvedChannel,
    event: &'a EventRefPayload,
    activity: &'a Activity,
    locator: &'a AttachmentSource,
    cap: u64,
    deadline: Instant,
) -> FetchRequest<'a> {
    FetchRequest {
        channel,
        event,
        activity,
        locator: Some(locator),
        declared_size: None,
        credential: SourceCredential::ReplyToken,
        cap,
        overflow: Overflow::Reject,
        deadline,
    }
}

#[derive(Clone, Default)]
pub struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Capture {
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// Capture this thread's logs under the gateway's filter built from `env`,
/// spans included, until the guard drops. `log` records (reqwest's) are
/// bridged into tracing exactly as the gateway's logger bridges them.
pub fn capture_logs(env: &str) -> (tracing::subscriber::DefaultGuard, Capture) {
    static LOG_BRIDGE: Once = Once::new();
    // While exactly one dispatcher is registered, tracing computes the
    // interest of a callsite first reached on another thread from *that*
    // thread's default, which in a shared test process is no subscriber at
    // all, and caches "never". A second dispatcher kept alive for the whole
    // process makes every callsite consult each live dispatcher instead.
    static KEEP_REGISTRY_SHARED: OnceLock<tracing::Dispatch> = OnceLock::new();
    KEEP_REGISTRY_SHARED
        .get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
    LOG_BRIDGE.call_once(|| tracing_log::LogTracer::init().expect("install the log bridge"));
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
            .with_writer(move || writer.clone())
            .with_filter(cmaeg::log_layer_filter(Some(env))),
    );
    (tracing::subscriber::set_default(subscriber), capture)
}

/// Assert that none of `secrets` appears in `text`.
pub fn assert_absent(text: &str, secrets: &[&str]) {
    for secret in secrets {
        assert!(!secret.is_empty());
        assert!(!text.contains(secret), "{secret:?} leaked into: {text}");
    }
}
