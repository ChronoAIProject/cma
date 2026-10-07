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
        transcribe::{
            meter::SttSpend, upload::NoTranscoder, AudioContainer, AudioInput, TranscribeError,
            Transcriber, Transcript,
        },
        visual::{PlatformThumbnail, VisualExtractor, VisualKind, VisualRequest},
        MediaLimits,
    },
    tools::MediaDownloads,
    MediaConfig, MediaToolProviders, SttConfig,
};
use cmaeg_core::{
    activity::{ActivityKind, Attachment, AttachmentKind, AttachmentSource, SourceKind},
    channel::Platform,
    seal::{event_ref::activity_digest, EventRef, MasterKeyRing, ReplyAddress},
};
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::{
    num::NonZeroU32,
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
    /// A custom event instead of a relayed message.
    pub custom: bool,
    /// The providers whose bindings install the gateway tools.
    pub tool_providers: Vec<&'static str>,
    /// A pod-wide mint rate below the production one.
    pub global_mints: Option<NonZeroU32>,
    /// Seal the event_ref with the longest partition key it can carry.
    pub longest_partition_key: bool,
    /// Seal the activity digest into the event_ref.
    pub digest: bool,
    /// Extract frames with the production platform-thumbnail extractor over
    /// the fake source instead of the fake extractor.
    pub thumbnails: bool,
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
            custom: false,
            tool_providers: vec!["cma_codex"],
            global_mints: None,
            longest_partition_key: false,
            digest: true,
            thumbnails: false,
            limits: MediaLimits::from_config(&MediaConfig::default()),
            edit: |_| {},
        }
    }

    pub fn native(mut self) -> Self {
        self.native = true;
        self
    }

    pub fn custom(mut self) -> Self {
        self.custom = true;
        self
    }

    pub fn tool_providers(mut self, providers: Vec<&'static str>) -> Self {
        self.tool_providers = providers;
        self
    }

    pub fn global_mints(mut self, per_minute: u32) -> Self {
        self.global_mints = NonZeroU32::new(per_minute);
        self
    }

    pub fn longest_partition_key(mut self) -> Self {
        self.longest_partition_key = true;
        self
    }

    /// The download state of an app built from this setup.
    pub fn downloads(&self) -> MediaDownloads {
        let downloads = MediaDownloads::default();
        match self.global_mints {
            Some(per_minute) => downloads.with_global_mint_rate(per_minute),
            None => downloads,
        }
    }

    pub fn platform_thumbnails(mut self) -> Self {
        self.thumbnails = true;
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
        if self.custom {
            request.activity.source = SourceKind::Custom;
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
        if self.custom {
            // A custom event without a result sink has no reply address.
            payload.reply = ReplyAddress::None;
        }
        if self.longest_partition_key {
            // Grow the key until the event_ref is at its size limit, which
            // leaves no room for the fields a media link adds.
            let mut length = 4_096;
            while length > 0 {
                payload.partition_key = PARTITION_MARKER.repeat(length / PARTITION_MARKER.len());
                if EventRef::seal(&payload, keys).is_ok() {
                    break;
                }
                length -= PARTITION_MARKER.len();
            }
        }
        request.event_ref = EventRef::seal(&payload, keys).unwrap();
        request
    }

    pub fn service(&self, keys: Arc<MasterKeyRing>) -> MediaService {
        let decode = Arc::new(Semaphore::new(1));
        let sources = self.fakes.source.registry();
        let visuals: Arc<dyn VisualExtractor> = if self.thumbnails {
            Arc::new(PlatformThumbnail::new(sources.clone(), decode.clone()))
        } else {
            self.fakes.visuals.clone()
        };
        // The direct upload path under the default spend limits, which no
        // test here comes near.
        let stt = SttConfig::default();
        let transcriber: Arc<dyn Transcriber> = self.fakes.transcriber.clone();
        MediaService::new(MediaDeps {
            sources,
            transcoder: Arc::new(NoTranscoder::new(&stt, transcriber.clone())),
            stt: Arc::new(SttSpend::new(&stt)),
            transcriber,
            visuals,
            limits: self.limits,
            keys,
            tool_providers: Arc::new(MediaToolProviders::from_iter(
                self.tool_providers.iter().map(|slug| slug.parse().unwrap()),
            )),
            decode,
        })
    }
}

/// The text a longest partition key repeats, so a test can find it in logs.
pub const PARTITION_MARKER: &str = "partition-marker-";

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
