//! Relayed attachments through the fake NyxID attachment route.
use super::{
    fake_nyxid::{AttachmentRequest, FakeNyxid},
    registry::stub,
    support::{
        activity, assert_absent, capture_logs, event, far, isolated, request, resolved, unix_now,
        TRACE_ENV,
    },
};
use bytes::Bytes;
use cmaeg::{
    channels::resolver::ResolvedChannel,
    media::{
        body::Overflow,
        source::{nyxid::NyxidSource, MediaSource, SourceCredential, SourceRegistry},
        FetchError,
    },
};
use cmaeg_core::{
    activity::{Activity, AttachmentSource, SourceKind},
    channel::{Platform, Secret},
    seal::{EventRefPayload, ReplyAddress},
};
use futures_util::StreamExt;
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant as StdInstant},
};
use tokio::time::Instant;

const REPLY_TOKEN: &str = "relay-reply-fixture";
const CHANNEL_KEY: &str = "relay-channel-fixture";
const MESSAGE_ID: &str = "nyx-message-7f3a";
const MIB: u64 = 1024 * 1024;

struct Relay {
    nyxid: FakeNyxid,
    source: Arc<NyxidSource>,
    channel: ResolvedChannel,
    event: EventRefPayload,
    activity: Activity,
    locator: AttachmentSource,
}

impl Relay {
    fn new(channel_key: bool, token_expires_at: i64) -> Self {
        let nyxid = isolated(FakeNyxid::start);
        let source = Arc::new(NyxidSource::new(&nyxid.base_url).unwrap());
        let channel = resolved(|definition| {
            definition.secrets.channel_key = channel_key.then(|| Secret::new(CHANNEL_KEY));
        });
        Self {
            nyxid,
            source,
            channel,
            event: event(relay_reply(token_expires_at)),
            activity: activity(
                SourceKind::NyxidRelay {
                    platform: Platform::Telegram,
                    route_id: "route-1".into(),
                },
                None,
            ),
            locator: AttachmentSource::NyxidRelay { index: 2 },
        }
    }

    fn live() -> Self {
        Self::new(true, unix_now() + 600)
    }

    async fn fetch(&self, credential: SourceCredential, cap: u64) -> Result<Bytes, FetchError> {
        self.fetch_by(credential, cap, far()).await
    }

    async fn fetch_by(
        &self,
        credential: SourceCredential,
        cap: u64,
        deadline: Instant,
    ) -> Result<Bytes, FetchError> {
        let mut request = request(
            &self.channel,
            &self.event,
            &self.activity,
            &self.locator,
            cap,
            deadline,
        );
        request.credential = credential;
        self.source.fetch(request).await.map(|body| body.bytes)
    }

    fn requests(&self) -> Vec<AttachmentRequest> {
        self.nyxid.attachments.requests.lock().unwrap().clone()
    }

    fn bearers(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|request| request.bearer)
            .collect()
    }
}

fn relay_reply(expires_at: i64) -> ReplyAddress {
    ReplyAddress::NyxidRelay {
        thread_reply: false,
        message_id: MESSAGE_ID.into(),
        route_id: "route-1".into(),
        platform: Platform::Telegram,
        reply_token: Secret::new(REPLY_TOKEN),
        reply_token_expires_at: expires_at,
    }
}

#[tokio::test]
async fn the_reply_token_fetches_the_attachment() {
    let relay = Relay::live();
    let bytes = relay
        .fetch(SourceCredential::ReplyToken, 25 * MIB)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"attachment-bytes");
    assert_eq!(
        relay.requests(),
        [AttachmentRequest {
            message_id: MESSAGE_ID.into(),
            index: "2".into(),
            bearer: REPLY_TOKEN.into(),
        }]
    );
}

#[tokio::test]
async fn a_refused_reply_token_is_retried_once_with_the_channel_key() {
    for status in [401, 403] {
        let relay = Relay::live();
        relay
            .nyxid
            .attachments
            .reply_status
            .store(status, Ordering::SeqCst);
        let bytes = relay
            .fetch(SourceCredential::ReplyToken, MIB)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"attachment-bytes");
        assert_eq!(relay.bearers(), [REPLY_TOKEN, CHANNEL_KEY], "{status}");
    }
}

#[tokio::test]
async fn a_refused_retry_is_unauthorized_after_exactly_two_requests() {
    let relay = Relay::live();
    let state = &relay.nyxid.attachments;
    state.reply_status.store(401, Ordering::SeqCst);
    state.key_status.store(403, Ordering::SeqCst);
    assert_eq!(
        relay.fetch(SourceCredential::ReplyToken, MIB).await,
        Err(FetchError::Unauthorized)
    );
    assert_eq!(relay.bearers(), [REPLY_TOKEN, CHANNEL_KEY]);
}

#[tokio::test]
async fn a_refused_reply_token_without_a_channel_key_is_unauthorized() {
    let relay = Relay::new(false, unix_now() + 600);
    relay
        .nyxid
        .attachments
        .reply_status
        .store(401, Ordering::SeqCst);
    assert_eq!(
        relay.fetch(SourceCredential::ReplyToken, MIB).await,
        Err(FetchError::Unauthorized)
    );
    assert_eq!(relay.bearers(), [REPLY_TOKEN]);
}

#[tokio::test]
async fn an_expired_reply_token_starts_with_the_channel_key() {
    let relay = Relay::new(true, unix_now() - 1);
    relay
        .fetch(SourceCredential::ReplyToken, MIB)
        .await
        .unwrap();
    assert_eq!(relay.bearers(), [CHANNEL_KEY]);
    let without_key = Relay::new(false, unix_now() - 1);
    assert_eq!(
        without_key.fetch(SourceCredential::ReplyToken, MIB).await,
        Err(FetchError::Unauthorized)
    );
    assert!(without_key.requests().is_empty());
}

#[tokio::test]
async fn the_channel_key_credential_never_sends_the_reply_token() {
    let relay = Relay::live();
    relay
        .fetch(SourceCredential::ChannelKey, MIB)
        .await
        .unwrap();
    assert_eq!(relay.bearers(), [CHANNEL_KEY]);
    let without_key = Relay::new(false, unix_now() + 600);
    assert_eq!(
        without_key.fetch(SourceCredential::ChannelKey, MIB).await,
        Err(FetchError::Unauthorized)
    );
    assert!(without_key.requests().is_empty());
}

#[tokio::test]
async fn statuses_are_classified() {
    for (status, expected) in [
        (404, FetchError::NotFound),
        (413, FetchError::TooLarge),
        (429, FetchError::RateLimited),
        (400, FetchError::Rejected),
        (503, FetchError::Busy),
        (500, FetchError::Upstream),
    ] {
        let relay = Relay::live();
        relay
            .nyxid
            .attachments
            .reply_status
            .store(status, Ordering::SeqCst);
        assert_eq!(
            relay.fetch(SourceCredential::ChannelKey, MIB).await,
            Ok(Bytes::from_static(b"attachment-bytes")),
            "the channel key is still accepted"
        );
        assert_eq!(
            relay.fetch(SourceCredential::ReplyToken, MIB).await,
            Err(expected),
            "{status}"
        );
    }
}

#[tokio::test]
async fn a_declared_length_over_twenty_mib_is_too_large_without_reading() {
    let relay = Relay::live();
    *relay.nyxid.attachments.declared_length.lock().unwrap() = Some(21 * MIB);
    relay
        .nyxid
        .attachments
        .delay_ms
        .store(10_000, Ordering::SeqCst);
    let started = StdInstant::now();
    assert_eq!(
        relay.fetch(SourceCredential::ReplyToken, 25 * MIB).await,
        Err(FetchError::TooLarge)
    );
    // Reading would have waited for the delayed body.
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn a_streamed_overflow_is_too_large() {
    let relay = Relay::live();
    let state = &relay.nyxid.attachments;
    *state.body.lock().unwrap() = Bytes::from(vec![7; 2 * MIB as usize]);
    state.chunked.store(true, Ordering::SeqCst);
    assert_eq!(
        relay.fetch(SourceCredential::ReplyToken, MIB).await,
        Err(FetchError::TooLarge)
    );
    // The same body fits a larger cap.
    assert_eq!(
        relay
            .fetch(SourceCredential::ReplyToken, 2 * MIB)
            .await
            .unwrap()
            .len(),
        2 * MIB as usize
    );
}

#[tokio::test]
async fn a_slow_body_past_the_deadline_is_timed_out() {
    let relay = Relay::live();
    relay
        .nyxid
        .attachments
        .delay_ms
        .store(5_000, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_millis(300);
    assert_eq!(
        relay
            .fetch_by(SourceCredential::ReplyToken, MIB, deadline)
            .await,
        Err(FetchError::TimedOut)
    );
}

#[tokio::test]
async fn a_non_relay_reply_address_is_not_retrievable() {
    let mut relay = Relay::live();
    relay.event = event(ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "1".into(),
        message_id: "1".into(),
        thread_id: None,
    });
    assert_eq!(
        relay.fetch(SourceCredential::ReplyToken, MIB).await,
        Err(FetchError::NotRetrievable)
    );
    relay.event = event(relay_reply(unix_now() + 600));
    relay.locator = AttachmentSource::Url {
        url: "https://media.example/a.png".into(),
    };
    assert_eq!(
        relay.fetch(SourceCredential::ReplyToken, MIB).await,
        Err(FetchError::NotRetrievable)
    );
    assert!(relay.requests().is_empty());
}

#[tokio::test]
async fn open_streams_the_attachment_under_the_same_caps() {
    let relay = Relay::live();
    let state = &relay.nyxid.attachments;
    *state.body.lock().unwrap() = Bytes::from(vec![7; 100_000]);
    let mut request = request(
        &relay.channel,
        &relay.event,
        &relay.activity,
        &relay.locator,
        MIB,
        far(),
    );
    let opened = relay.source.open(request).await.unwrap();
    assert_eq!(opened.content_length, Some(100_000));
    let chunks: Vec<_> = opened.stream.collect().await;
    assert_eq!(
        chunks
            .into_iter()
            .map(|chunk| chunk.unwrap().len())
            .sum::<usize>(),
        100_000
    );
    request.cap = 50_000;
    request.overflow = Overflow::Truncate;
    assert_eq!(
        relay.source.open(request).await.unwrap_err(),
        FetchError::TooLarge
    );
}

#[tokio::test]
async fn neither_credentials_nor_the_message_or_path_reach_logs_or_errors() {
    let (guard, logs) = capture_logs(TRACE_ENV);
    let relay = Relay::live();
    relay
        .nyxid
        .attachments
        .reply_status
        .store(401, Ordering::SeqCst);
    relay
        .fetch(SourceCredential::ReplyToken, MIB)
        .await
        .unwrap();
    relay
        .nyxid
        .attachments
        .key_status
        .store(500, Ordering::SeqCst);
    let error = relay
        .fetch(SourceCredential::ReplyToken, MIB)
        .await
        .unwrap_err();
    // Through the registry too, so its span and events are judged.
    let registry = SourceRegistry::new(relay.source.clone(), stub(), stub(), stub());
    let request = request(
        &relay.channel,
        &relay.event,
        &relay.activity,
        &relay.locator,
        MIB,
        far(),
    );
    assert_eq!(
        registry.fetch(request).await.unwrap_err(),
        FetchError::Upstream
    );
    relay
        .nyxid
        .attachments
        .key_status
        .store(200, Ordering::SeqCst);
    relay
        .nyxid
        .attachments
        .delay_ms
        .store(5_000, Ordering::SeqCst);
    let timed_out = relay
        .fetch_by(
            SourceCredential::ChannelKey,
            MIB,
            Instant::now() + Duration::from_millis(200),
        )
        .await
        .unwrap_err();
    drop(guard);
    let text = logs.text();
    assert!(
        text.contains("retrying once with the channel key"),
        "{text}"
    );
    assert!(text.contains("cmaeg.media.fetch"), "{text}");
    assert!(text.contains("outcome=\"upstream\""), "{text}");
    assert_absent(
        &text,
        &[
            REPLY_TOKEN,
            CHANNEL_KEY,
            MESSAGE_ID,
            "/api/v1/channel-relay",
            "attachments/2",
        ],
    );
    for shown in [
        format!("{error:?}"),
        format!("{timed_out:?}"),
        error.to_string(),
    ] {
        assert_absent(&shown, &["http", "127.0.0.1", MESSAGE_ID, REPLY_TOKEN]);
    }
    assert_eq!(
        (error, timed_out),
        (FetchError::Upstream, FetchError::TimedOut)
    );
}
