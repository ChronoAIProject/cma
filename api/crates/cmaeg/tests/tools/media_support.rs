//! A media-enabled tools app: an event whose activity carries attachments,
//! sealed into its event_ref, served by a media service over in-memory
//! sources, a fake transcriber and a fake visual extractor.
use super::media::fakes::{attachment, FakeSource};
use bytes::Bytes;
use cmaeg::{
    ingress::launch::LaunchRequest,
    media::{
        derive::DeriveError,
        image::NormalizedImage,
        service::{MediaDeps, MediaService},
        transcribe::{AudioContainer, AudioInput, TranscribeError, Transcriber, Transcript},
        visual::{VisualExtractor, VisualKind, VisualRequest},
        MediaLimits,
    },
    MediaConfig, MediaToolProviders,
};
use cmaeg_core::{
    activity::{ActivityKind, Attachment, AttachmentKind, AttachmentSource, SourceKind},
    channel::Platform,
    seal::{event_ref::activity_digest, EventRef, MasterKeyRing},
};
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{sync::Semaphore, time::Instant};

/// Returns a fixed text after an optional delay, counting its calls.
pub struct FakeTranscriber {
    enabled: AtomicBool,
    text: Mutex<String>,
    delay: Mutex<Duration>,
    calls: AtomicUsize,
}

impl Default for FakeTranscriber {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            text: Mutex::new("fake transcript".into()),
            delay: Mutex::new(Duration::ZERO),
            calls: AtomicUsize::new(0),
        }
    }
}

impl FakeTranscriber {
    pub fn disable(&self) {
        self.enabled.store(false, Ordering::SeqCst);
    }

    pub fn say(&self, text: &str) {
        *self.text.lock().unwrap() = text.into();
    }

    pub fn delay(&self, delay: Duration) {
        *self.delay.lock().unwrap() = delay;
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Transcriber for FakeTranscriber {
    fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    fn accepts(&self, _container: AudioContainer) -> bool {
        true
    }

    fn transcribe<'a>(
        &'a self,
        _input: AudioInput,
        _deadline: Instant,
    ) -> BoxFuture<'a, Result<Transcript, TranscribeError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = self.text.lock().unwrap().clone();
        let delay = *self.delay.lock().unwrap();
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Ok(Transcript {
                text,
                usage_seconds: Some(1),
            })
        })
    }
}

/// A frame the fake visual extractor returns: `len` bytes that start like
/// a JPEG, at `at_ms` in the video.
#[derive(Clone)]
pub struct FrameSpec {
    pub len: usize,
    pub at_ms: Option<u64>,
}

/// Returns configured frames, or a configured failure (by default
/// `not_retrievable`, like a relayed video without a thumbnail).
pub struct FakeVisuals {
    kind: Mutex<VisualKind>,
    frames: Mutex<Result<Vec<FrameSpec>, DeriveError>>,
    calls: AtomicUsize,
}

impl Default for FakeVisuals {
    fn default() -> Self {
        Self {
            kind: Mutex::new(VisualKind::Thumbnail),
            frames: Mutex::new(Err(DeriveError::NotRetrievable)),
            calls: AtomicUsize::new(0),
        }
    }
}

impl FakeVisuals {
    pub fn serve(&self, frames: Vec<FrameSpec>) {
        *self.frames.lock().unwrap() = Ok(frames);
    }

    pub fn keyframes(&self) {
        *self.kind.lock().unwrap() = VisualKind::Keyframes;
    }
}

/// A preview-sized JPEG-looking frame of exactly `len` bytes.
pub fn frame_image(spec: &FrameSpec) -> NormalizedImage {
    let mut bytes = vec![0_u8; spec.len];
    bytes[..2].copy_from_slice(&[0xFF, 0xD8]);
    NormalizedImage {
        bytes: Bytes::from(bytes),
        mime: "image/jpeg",
        width: 768,
        height: 432,
        at_ms: spec.at_ms,
    }
}

impl VisualExtractor for FakeVisuals {
    fn kind(&self) -> VisualKind {
        *self.kind.lock().unwrap()
    }

    fn extract<'a>(
        &'a self,
        _request: VisualRequest<'a>,
    ) -> BoxFuture<'a, Result<Vec<NormalizedImage>, DeriveError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let frames = self.frames.lock().unwrap().clone();
        Box::pin(async move { Ok(frames?.iter().map(frame_image).collect()) })
    }
}

/// The fakes behind one app's media service.
#[derive(Clone, Default)]
pub struct MediaFakes {
    pub source: Arc<FakeSource>,
    pub transcriber: Arc<FakeTranscriber>,
    pub visuals: Arc<FakeVisuals>,
}

/// What a media-enabled app's event carries and how it is sealed.
pub struct MediaSetup {
    pub fakes: MediaFakes,
    pub attachments: Vec<Attachment>,
    /// A natively received Telegram message instead of a relayed one.
    pub native: bool,
    /// Seal the activity digest into the event_ref.
    pub digest: bool,
    pub limits: MediaLimits,
    /// Applied to the channel definition before it is stored.
    pub edit: fn(&mut Value),
}

impl MediaSetup {
    pub fn new(attachments: Vec<Attachment>) -> Self {
        Self {
            fakes: MediaFakes::default(),
            attachments,
            native: false,
            digest: true,
            limits: MediaLimits::from_config(&MediaConfig::default()),
            edit: |_| {},
        }
    }

    pub fn native(mut self) -> Self {
        self.native = true;
        self
    }

    pub fn without_digest(mut self) -> Self {
        self.digest = false;
        self
    }

    pub fn limits(mut self, change: impl FnOnce(&mut MediaLimits)) -> Self {
        change(&mut self.limits);
        self
    }

    pub fn edit(mut self, edit: fn(&mut Value)) -> Self {
        self.edit = edit;
        self
    }

    /// `request` with this setup's activity, under an event_ref resealed
    /// with that activity's digest (or with none).
    pub fn retarget(&self, mut request: LaunchRequest, keys: &MasterKeyRing) -> LaunchRequest {
        if self.native {
            request.activity.source = SourceKind::Telegram {
                platform: Platform::Telegram,
                bot_id: "77".into(),
            };
        }
        request.activity.kind = ActivityKind::Message {
            text: "see attached".into(),
            attachments: self.attachments.clone(),
            mentions_bot: true,
            reply_to: None,
            album_id: None,
        };
        let mut payload = request
            .event_ref
            .open(keys, super::relay_support::now())
            .unwrap()
            .payload;
        payload.activity_sha256 = self.digest.then(|| activity_digest(&request.activity));
        request.event_ref = EventRef::seal(&payload, keys).unwrap();
        request
    }

    pub fn service(&self, keys: Arc<MasterKeyRing>) -> MediaService {
        let decode = Arc::new(Semaphore::new(1));
        MediaService::new(MediaDeps {
            sources: self.fakes.source.registry(),
            transcriber: self.fakes.transcriber.clone(),
            visuals: self.fakes.visuals.clone(),
            limits: self.limits,
            keys,
            tool_providers: Arc::new(MediaToolProviders::from_iter(["cma_codex"
                .parse()
                .unwrap()])),
            decode,
        })
    }
}

/// A relayed attachment of `kind` at callback position `index`.
pub fn relayed(index: u8, kind: AttachmentKind) -> Attachment {
    let mut item = attachment(kind, Some(AttachmentSource::NyxidRelay { index }));
    item.reference = format!("telegram:item-{index}");
    item
}

/// A natively received Telegram attachment of `kind`.
pub fn native(unique: &str, kind: AttachmentKind) -> Attachment {
    let mut item = attachment(
        kind,
        Some(AttachmentSource::Telegram {
            file_id: format!("file-{unique}"),
            file_unique_id: unique.into(),
        }),
    );
    item.reference = format!("telegram:{unique}");
    item
}

/// An MP3 head the sniffer recognizes as audio.
pub fn mp3() -> Vec<u8> {
    let mut bytes = b"ID3\x04\0\0\0\0\0\0".to_vec();
    bytes.resize(2048, 0);
    bytes
}
