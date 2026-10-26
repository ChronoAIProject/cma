//! Locator dispatch, source matching and recording in the source registry.
use super::support::{activity, capture_logs, event, far, request, resolved, TRACE_ENV};
use bytes::Bytes;
use cmaeg::{
    media::{
        body::FetchedBody,
        source::{FetchRequest, MediaSource, SourceRegistry, SourceStream},
        FetchError,
    },
    telemetry::channel_hash,
};
use cmaeg_core::{
    activity::{AttachmentSource, SourceKind},
    channel::Platform,
    seal::ReplyAddress,
};
use futures_util::{future::BoxFuture, StreamExt};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// A source answering with fixed bytes and counting its calls.
pub struct Stub {
    bytes: &'static [u8],
    calls: AtomicUsize,
}

impl MediaSource for Stub {
    fn fetch<'a>(
        &'a self,
        _request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchedBody, FetchError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(FetchedBody {
                bytes: Bytes::from_static(self.bytes),
                truncated: false,
            })
        })
    }

    fn open<'a>(
        &'a self,
        _request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<SourceStream, FetchError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(SourceStream {
                content_length: None,
                stream: futures_util::stream::iter([
                    Ok(Bytes::from_static(self.bytes)),
                    Ok(Bytes::from_static(self.bytes)),
                ])
                .boxed(),
            })
        })
    }
}

fn named(bytes: &'static [u8]) -> Arc<Stub> {
    Arc::new(Stub {
        bytes,
        calls: AtomicUsize::new(0),
    })
}

/// A source the test does not expect to reach.
pub fn stub() -> Arc<dyn MediaSource> {
    named(b"stub")
}

struct Sources {
    nyxid: Arc<Stub>,
    telegram: Arc<Stub>,
    url: Arc<Stub>,
    inline: Arc<Stub>,
    registry: SourceRegistry,
}

fn sources() -> Sources {
    let (nyxid, telegram, url, inline) = (
        named(b"nyxid"),
        named(b"telegram"),
        named(b"url"),
        named(b"inline"),
    );
    let registry =
        SourceRegistry::new(nyxid.clone(), telegram.clone(), url.clone(), inline.clone());
    Sources {
        nyxid,
        telegram,
        url,
        inline,
        registry,
    }
}

fn kinds() -> [(SourceKind, &'static str); 3] {
    [
        (
            SourceKind::NyxidRelay {
                platform: Platform::Telegram,
                route_id: "route-1".into(),
            },
            "relay",
        ),
        (
            SourceKind::Telegram {
                platform: Platform::Telegram,
                bot_id: "77".into(),
            },
            "telegram",
        ),
        (SourceKind::Custom, "custom"),
    ]
}

fn locators() -> [(AttachmentSource, &'static str); 4] {
    [
        (AttachmentSource::NyxidRelay { index: 0 }, "relay"),
        (
            AttachmentSource::Telegram {
                file_id: "file-1".into(),
                file_unique_id: "unique-1".into(),
            },
            "telegram",
        ),
        (
            AttachmentSource::Url {
                url: "https://media.example/a.png".into(),
            },
            "custom",
        ),
        (
            AttachmentSource::Inline {
                pointer: "/attachments/0/url".into(),
            },
            "custom",
        ),
    ]
}

#[tokio::test]
async fn each_locator_reaches_only_its_source_and_only_with_its_activity_source() {
    let channel = resolved(|_| {});
    let event = event(ReplyAddress::None);
    for (kind, kind_name) in kinds() {
        let activity = activity(kind, None);
        for (locator, locator_kind) in locators() {
            let sources = sources();
            let result = sources
                .registry
                .fetch(request(&channel, &event, &activity, &locator, 1024, far()))
                .await;
            let calls = [
                &sources.nyxid,
                &sources.telegram,
                &sources.url,
                &sources.inline,
            ]
            .map(|source| source.calls.load(Ordering::SeqCst));
            if locator_kind == kind_name {
                let expected: &[u8] = match locator {
                    AttachmentSource::Discord { .. } => b"discord",
                    AttachmentSource::NyxidRelay { .. } => b"nyxid",
                    AttachmentSource::Telegram { .. } => b"telegram",
                    AttachmentSource::Url { .. } => b"url",
                    AttachmentSource::Inline { .. } => b"inline",
                };
                assert_eq!(&result.unwrap().bytes[..], expected);
                assert_eq!(calls.iter().sum::<usize>(), 1);
            } else {
                assert_eq!(result.unwrap_err(), FetchError::NotRetrievable);
                assert_eq!(calls, [0; 4], "{kind_name} with {locator:?}");
            }
        }
    }
}

#[tokio::test]
async fn an_attachment_without_a_locator_is_not_retrievable() {
    let sources = sources();
    let channel = resolved(|_| {});
    let event = event(ReplyAddress::None);
    let activity = activity(SourceKind::Custom, None);
    let locator = AttachmentSource::NyxidRelay { index: 0 };
    let mut request = request(&channel, &event, &activity, &locator, 1024, far());
    request.locator = None;
    assert_eq!(
        sources.registry.fetch(request).await.unwrap_err(),
        FetchError::NotRetrievable
    );
    assert_eq!(
        sources.registry.open(request).await.unwrap_err(),
        FetchError::NotRetrievable
    );
}

#[tokio::test]
async fn fetches_and_opened_streams_are_recorded_by_source_and_outcome() {
    // A recorder local to this test's thread, so no other test's fetches count.
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let sources = sources();
    let channel = resolved(|_| {});
    let event = event(ReplyAddress::None);
    let activity = activity(SourceKind::Custom, None);
    let (url, _) = locators()[2].clone();
    let (relay, _) = locators()[0].clone();
    let fetched = sources
        .registry
        .fetch(request(&channel, &event, &activity, &url, 1024, far()))
        .await
        .unwrap();
    assert_eq!(&fetched.bytes[..], b"url");
    let opened = sources
        .registry
        .open(request(&channel, &event, &activity, &url, 1024, far()))
        .await
        .unwrap();
    assert_eq!(opened.stream.count().await, 2);
    assert!(sources
        .registry
        .fetch(request(&channel, &event, &activity, &relay, 1024, far()))
        .await
        .is_err());
    let text = handle.render();
    for line in [
        "cmaeg_media_fetch_seconds_count{source=\"url\",outcome=\"ok\"} 2",
        "cmaeg_media_fetch_seconds_count{source=\"nyxid\",outcome=\"not_retrievable\"} 1",
        "cmaeg_media_fetch_bytes_total{source=\"url\"} 9",
    ] {
        assert!(
            text.lines().any(|candidate| candidate == line),
            "{line}: {text}"
        );
    }
}

#[tokio::test]
async fn the_fetch_span_names_the_channel_only_by_its_hash() {
    let (guard, logs) = capture_logs(TRACE_ENV);
    let sources = sources();
    let channel = resolved(|_| {});
    let event = event(ReplyAddress::None);
    let activity = activity(SourceKind::Custom, None);
    let (url, _) = locators()[2].clone();
    sources
        .registry
        .fetch(request(&channel, &event, &activity, &url, 1024, far()))
        .await
        .unwrap();
    drop(guard);
    let text = logs.text();
    let hash = channel_hash(channel.id.as_str());
    assert!(
        text.contains(&format!("cmaeg.media.fetch{{source=\"url\" channel={hash}")),
        "{text}"
    );
    assert!(text.contains("outcome=\"ok\" bytes=3"), "{text}");
    assert!(!text.contains(channel.id.as_str()), "{text}");
    assert!(!text.contains("media.example"), "{text}");
}
