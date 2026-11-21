//! Real preparation pipeline with a bounded in-memory attachment source.
use super::test_support::fixture;
use crate::{
    media::{
        body::FetchedBody,
        budget::InflightBytes,
        service::{MediaDeps, MediaService},
        source::{FetchRequest, MediaSource, SourceRegistry, SourceStream},
        transcribe::{meter::SttSpend, upload::NoTranscoder, DisabledTranscriber},
        visual::PlatformThumbnail,
        FetchError, MediaLimits,
    },
    MediaConfig, MediaToolProviders, SttConfig,
};
use cmaeg_core::activity::{Attachment, AttachmentKind, AttachmentSource};
use futures_util::future::BoxFuture;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Semaphore;

#[derive(Default)]
pub struct TextSource {
    pub calls: AtomicUsize,
}
impl MediaSource for TextSource {
    fn fetch<'a>(&'a self, _: FetchRequest<'a>) -> BoxFuture<'a, Result<FetchedBody, FetchError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(FetchedBody {
                bytes: bytes::Bytes::from_static(b"shared media excerpt for the routed consumer"),
                truncated: false,
            })
        })
    }
    fn open<'a>(&'a self, _: FetchRequest<'a>) -> BoxFuture<'a, Result<SourceStream, FetchError>> {
        Box::pin(async { Err(FetchError::Rejected) })
    }
}
pub fn service() -> (Arc<MediaService>, Arc<TextSource>) {
    let source = Arc::new(TextSource::default());
    let registry = Arc::new(SourceRegistry::new(
        source.clone(),
        source.clone(),
        source.clone(),
        source.clone(),
    ));
    let transcriber = Arc::new(DisabledTranscriber);
    let stt = SttConfig::default();
    let limits = MediaLimits::from_config(&MediaConfig::default());
    let decode = Arc::new(Semaphore::new(1));
    let service = Arc::new(MediaService::new(MediaDeps {
        visuals: Arc::new(PlatformThumbnail::new(registry.clone(), decode.clone())),
        sources: registry,
        transcriber: transcriber.clone(),
        transcoder: Arc::new(NoTranscoder::new(&stt, transcriber)),
        stt: Arc::new(SttSpend::new(&stt)),
        limits,
        keys: Arc::new(fixture::keys()),
        tool_providers: Arc::new(MediaToolProviders::from_iter(["cma_codex"
            .parse()
            .unwrap()])),
        decode,
        inflight: InflightBytes::new(&limits),
    }));
    (service, source)
}
pub fn attachment() -> Attachment {
    Attachment {
        reference: "original-attachment".into(),
        kind: AttachmentKind::File,
        filename: Some("notes.txt".into()),
        mime_type: Some("text/plain".into()),
        size_bytes: Some(44),
        source: Some(AttachmentSource::Telegram {
            file_id: "private_file_locator".into(),
            file_unique_id: "unique_fixture".into(),
        }),
        variant: None,
        duration_seconds: None,
        width: None,
        height: None,
        thumbnail: None,
        emoji: None,
    }
}
