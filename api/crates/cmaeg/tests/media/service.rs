//! The media service's per-artifact pipelines over in-memory sources:
//! images, text, transcripts, visuals, the cache and failure reasons.
use super::{
    fakes::{
        attachment, custom_target, message, target, telegram_file, telegram_source, url,
        FakeSource, Seen,
    },
    image_fixtures as fixtures,
    support::capture_logs,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg::{
    media::{
        body::Overflow,
        derive::{Artifact, DeriveError, Derived},
        jobs::JobOutcome,
        service::{MediaCapabilities, MediaDeps, MediaService},
        sniff::Family,
        source::SourceCredential,
        target::MediaTarget,
        text::{EXCERPT_BYTES, EXCERPT_READ_LIMIT, TEXT_READ_LIMIT},
        transcribe::{
            AudioContainer, AudioInput, DisabledTranscriber, TranscribeError, Transcriber,
            Transcript,
        },
        visual::{PlatformThumbnail, VisualKind},
        MediaLimits,
    },
    MediaConfig, MediaToolProviders,
};
use cmaeg_core::{activity::AttachmentKind, seal::MasterKeyRing};
use futures_util::future::BoxFuture;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{sync::Semaphore, time::Instant};

pub(super) const REPLY_TOKEN: &str = "service-reply-token-never-shown";
pub(super) const LONG: Duration = Duration::from_secs(60);

pub(super) fn ring() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([7; 32]))).unwrap()
}

pub(super) fn limits() -> MediaLimits {
    MediaLimits::from_config(&MediaConfig::default())
}

/// Accepts every container and returns `text`, counting its calls.
#[derive(Default)]
struct FakeTranscriber {
    text: &'static str,
    calls: AtomicUsize,
}

impl Transcriber for FakeTranscriber {
    fn enabled(&self) -> bool {
        true
    }

    fn accepts(&self, _container: AudioContainer) -> bool {
        true
    }

    fn transcribe<'a>(
        &'a self,
        input: AudioInput,
        _deadline: Instant,
    ) -> BoxFuture<'a, Result<Transcript, TranscribeError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(input.container, AudioContainer::Mp3);
        Box::pin(async move {
            Ok(Transcript {
                text: self.text.into(),
                usage_seconds: Some(7),
            })
        })
    }
}

pub(super) fn service_with(
    source: &Arc<FakeSource>,
    limits: MediaLimits,
    transcriber: Arc<dyn Transcriber>,
) -> MediaService {
    let decode = Arc::new(Semaphore::new(1));
    let registry = source.registry();
    MediaService::new(MediaDeps {
        visuals: Arc::new(PlatformThumbnail::new(registry.clone(), decode.clone())),
        sources: registry,
        transcriber,
        limits,
        keys: Arc::new(ring()),
        tool_providers: Arc::new(MediaToolProviders::default()),
        decode,
    })
}

pub(super) fn service(source: &Arc<FakeSource>) -> MediaService {
    service_with(source, limits(), Arc::new(DisabledTranscriber))
}

pub(super) fn ready(outcome: JobOutcome<Derived, DeriveError>) -> Arc<Derived> {
    match outcome {
        JobOutcome::Ready(value) => value,
        other => panic!("not ready: {other:?}"),
    }
}

pub(super) fn failed(outcome: JobOutcome<Derived, DeriveError>) -> DeriveError {
    match outcome {
        JobOutcome::Failed(error) => error,
        other => panic!("not failed: {other:?}"),
    }
}

pub(super) fn is_pending(outcome: &JobOutcome<Derived, DeriveError>) -> bool {
    matches!(outcome, JobOutcome::Pending { .. })
}

/// A custom message with one file per name, each served from its URL.
pub(super) fn files(source: &FakeSource, served: &[(&str, Vec<u8>)]) -> MediaTarget {
    let attachments = served
        .iter()
        .map(|(name, bytes)| {
            source.serve(&url(name), Ok(bytes.clone()));
            attachment(AttachmentKind::File, Some(url(name)))
        })
        .collect();
    custom_target(attachments)
}

#[tokio::test]
async fn an_image_is_normalized_to_the_full_budget() {
    let source = FakeSource::new();
    let photo = fixtures::jpeg(&fixtures::rgb(3000, 2000, 16), 90);
    let target = files(&source, &[("photo", photo)]);
    let derived = ready(
        service(&source)
            .derive(&target, 0, Artifact::Image, LONG)
            .await,
    );
    let Derived::Image(image) = &*derived else {
        panic!("{derived:?}");
    };
    assert_eq!((image.width, image.height), (2048, 1365));
    assert!(image.bytes.len() <= 1_048_576);
    let max = MediaConfig::default().max_bytes;
    assert_eq!(
        source.seen(),
        [Seen {
            cap: max,
            overflow: Overflow::Reject,
            credential: SourceCredential::ReplyToken,
        }]
    );
}

#[tokio::test]
async fn text_is_paged_and_excerpts_are_bounded() {
    let source = FakeSource::new();
    let target = files(&source, &[("notes.txt", "line\r\n".repeat(7_000).into())]);
    let service = service(&source);
    let Derived::Text(doc) = &*ready(service.derive(&target, 0, Artifact::Text, LONG).await) else {
        panic!("not text");
    };
    assert_eq!(doc.pages.len(), 3);
    assert!(!doc.pages.concat().contains('\r'));
    let excerpt = ready(service.derive(&target, 0, Artifact::Excerpt, LONG).await);
    let Derived::Excerpt { text, truncated } = &*excerpt else {
        panic!("{excerpt:?}");
    };
    assert!(text.len() <= EXCERPT_BYTES && *truncated);
    let caps: Vec<_> = source
        .seen()
        .iter()
        .map(|seen| (seen.cap, seen.overflow))
        .collect();
    assert_eq!(
        caps,
        [
            (TEXT_READ_LIMIT, Overflow::Truncate),
            (EXCERPT_READ_LIMIT, Overflow::Truncate)
        ]
    );
}

fn mp3() -> Vec<u8> {
    let mut bytes = b"ID3\x04\0\0\0\0\0\0".to_vec();
    bytes.resize(2048, 0);
    bytes
}

#[tokio::test]
async fn audio_is_transcribed_normalized_and_paged() {
    let source = FakeSource::new();
    let target = files(&source, &[("voice", mp3())]);
    let transcriber = Arc::new(FakeTranscriber {
        text: "line one\r\nline\u{202E} two\u{7}",
        ..FakeTranscriber::default()
    });
    let service = service_with(&source, limits(), transcriber.clone());
    assert!(service.capabilities().transcriber_enabled);
    let derived = ready(service.derive(&target, 0, Artifact::Transcript, LONG).await);
    let Derived::Transcript {
        document,
        usage_seconds,
    } = &*derived
    else {
        panic!("{derived:?}");
    };
    assert_eq!(document.pages, ["line one\nline two"]);
    assert_eq!(*usage_seconds, Some(7));
    // Text is not audio, whatever the transcriber accepts.
    let text = files(&source, &[("not-audio", b"just words".to_vec())]);
    assert_eq!(
        failed(service.derive(&text, 0, Artifact::Transcript, LONG).await),
        DeriveError::UnsupportedFormat
    );
    assert_eq!(transcriber.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_disabled_transcriber_refuses_without_fetching() {
    let source = FakeSource::new();
    let target = files(&source, &[("voice", mp3())]);
    let service = service(&source);
    assert_eq!(
        failed(service.derive(&target, 0, Artifact::Transcript, LONG).await),
        DeriveError::TranscriberDisabled
    );
    service.warm(&target, 0, Artifact::Transcript);
    tokio::task::yield_now().await;
    assert_eq!(source.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_slow_source_answers_pending_and_then_from_the_cache() {
    let source = FakeSource::new();
    source.delay(Duration::from_secs(5));
    let target = files(&source, &[("slow.txt", b"eventually".to_vec())]);
    let service = service(&source);
    let first = service
        .derive(&target, 0, Artifact::Text, Duration::from_secs(1))
        .await;
    assert!(is_pending(&first), "{first:?}");
    tokio::time::sleep(Duration::from_secs(10)).await;
    ready(
        service
            .derive(&target, 0, Artifact::Text, Duration::from_secs(1))
            .await,
    );
    assert_eq!(source.calls(), 1);
}

#[tokio::test]
async fn the_sniffed_type_overrides_the_declared_kind_both_ways() {
    let source = FakeSource::new();
    let png = fixtures::png_rgb8(&fixtures::rgb(64, 64, 0));
    source.serve(&url("fake.png"), Ok(b"I am text".to_vec()));
    source.serve(&url("notes.txt"), Ok(png));
    let mut declared_image = attachment(AttachmentKind::Image, Some(url("fake.png")));
    declared_image.mime_type = Some("image/png".into());
    let mut declared_text = attachment(AttachmentKind::File, Some(url("notes.txt")));
    declared_text.mime_type = Some("text/plain".into());
    let target = custom_target(vec![declared_image, declared_text]);
    let service = service(&source);
    assert_eq!(
        failed(service.derive(&target, 0, Artifact::Image, LONG).await),
        DeriveError::UnsupportedFormat
    );
    ready(service.derive(&target, 0, Artifact::Text, LONG).await);
    ready(service.derive(&target, 1, Artifact::Image, LONG).await);
    assert_eq!(
        failed(service.derive(&target, 1, Artifact::Text, LONG).await),
        DeriveError::UnsupportedFormat
    );
    let Derived::Metadata { mime, family } =
        *ready(service.derive(&target, 1, Artifact::Metadata, LONG).await)
    else {
        panic!("not metadata");
    };
    assert_eq!((mime, family), ("image/png", Family::Image));
    assert_eq!(source.seen().last().unwrap().cap, 4096);
}

#[tokio::test]
async fn an_index_out_of_range_is_not_found_without_fetching() {
    let source = FakeSource::new();
    let target = files(&source, &[("a", b"a".to_vec())]);
    let service = service(&source);
    for index in [1, 300] {
        assert_eq!(
            failed(service.derive(&target, index, Artifact::Text, LONG).await),
            DeriveError::NotFound
        );
    }
    assert_eq!(source.calls(), 0);
}

#[tokio::test]
async fn fetch_failures_reach_the_caller_as_reasons() {
    let source = FakeSource::new();
    source.serve(&url("gone"), Err(cmaeg::media::FetchError::Upstream));
    let target = custom_target(vec![attachment(AttachmentKind::File, Some(url("gone")))]);
    let (_guard, logs) = capture_logs("cmaeg=debug");
    assert_eq!(
        failed(
            service(&source)
                .derive(&target, 0, Artifact::Text, LONG)
                .await
        ),
        DeriveError::SourceUnavailable
    );
    let text = logs.text();
    assert!(text.contains("cmaeg.media.derive"), "{text}");
    assert!(text.contains("outcome=\"source_unavailable\""), "{text}");
    assert!(!text.contains("files.example"), "{text}");
}

#[tokio::test(start_paused = true)]
async fn a_job_stopped_at_its_cap_records_timed_out() {
    let source = FakeSource::new();
    source.delay(Duration::from_secs(600));
    let target = files(&source, &[("stuck", b"never".to_vec())]);
    let (_guard, logs) = capture_logs("cmaeg=debug");
    let outcome = service(&source)
        .derive(&target, 0, Artifact::Text, Duration::from_secs(120))
        .await;
    assert_eq!(failed(outcome), DeriveError::TimedOut);
    let text = logs.text();
    assert!(text.contains("outcome=\"timed_out\""), "{text}");
}

#[tokio::test]
async fn platform_thumbnails_are_the_baseline_visuals() {
    let source = FakeSource::new();
    source.serve(
        &telegram_file("thumb"),
        Ok(fixtures::jpeg(&fixtures::rgb(320, 180, 8), 90)),
    );
    let mut video = attachment(AttachmentKind::Video, Some(telegram_file("video")));
    video.thumbnail = Some(telegram_file("thumb"));
    let target = target(telegram_source(), message(vec![video]));
    let service = service(&source);
    assert_eq!(
        service.capabilities(),
        MediaCapabilities {
            transcriber_enabled: false,
            visuals: VisualKind::Thumbnail,
        }
    );
    let Derived::Visuals(frames) =
        &*ready(service.derive(&target, 0, Artifact::Visuals, LONG).await)
    else {
        panic!("not visuals");
    };
    assert_eq!(frames.len(), 1);
}
