//! A real media service over counting fakes, behind a turn runner that
//! prepares media turns.
use super::{fake_provider::FakeProviderConfig, support::Fixture};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use bytes::Bytes;
use cmaeg::{
    channels::resolver::ResolvedChannel,
    ingress::launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
    media::{
        body::{FetchedBody, Overflow},
        budget::InflightBytes,
        derive::DeriveError,
        prepare::PrepareConfig,
        service::{MediaDeps, MediaService},
        sniff::Sniffed,
        source::{FetchRequest, MediaSource, SourceRegistry, SourceStream},
        transcribe::{
            meter::SttSpend,
            upload::{AudioTranscoder, NoTranscoder, PlannedUpload},
            AudioContainer, AudioInput, DisabledTranscriber, TranscribeError, Transcriber,
            Transcript,
        },
        transform::{
            adapters::TransformerTranscoder, AudioRequest, KeyframeRequest, Keyframes,
            MediaTransformer, Pcm, TransformError,
        },
        visual::PlatformThumbnail,
        FetchError, MediaLimits,
    },
    turns::TurnRunner,
    MediaConfig, MediaToolProviders, SttConfig, TurnConfig,
};
use cmaeg_core::{
    activity::{ActivityKind, Attachment, AttachmentKind, AttachmentSource, MediaVariant},
    channel::ChannelDefinition,
    ids::{ChannelId, ProviderSlug},
    seal::MasterKeyRing,
};
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{sync::Semaphore, time::Instant};

pub const WORDS: &str = "please ship the release on friday";

/// Serves relay attachment bodies by index after an optional delay, and
/// counts every fetch. An unknown index is `not_found`.
#[derive(Default)]
pub struct Source {
    bodies: Mutex<HashMap<u8, Result<Bytes, FetchError>>>,
    delay: Mutex<Duration>,
    calls: AtomicUsize,
}

impl Source {
    pub fn serve(&self, index: u8, body: Result<Vec<u8>, FetchError>) {
        self.bodies
            .lock()
            .unwrap()
            .insert(index, body.map(Bytes::from));
    }
    pub fn delay(&self, delay: Duration) {
        *self.delay.lock().unwrap() = delay;
    }
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl MediaSource for Source {
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchedBody, FetchError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let delay = *self.delay.lock().unwrap();
        let body = match request.locator {
            Some(AttachmentSource::NyxidRelay { index }) => {
                self.bodies.lock().unwrap().get(index).cloned()
            }
            _ => None,
        };
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            let bytes = body.unwrap_or(Err(FetchError::NotFound))?;
            let cap = usize::try_from(request.cap).unwrap();
            if bytes.len() <= cap {
                return Ok(FetchedBody {
                    bytes,
                    truncated: false,
                });
            }
            match request.overflow {
                Overflow::Reject => Err(FetchError::TooLarge),
                Overflow::Truncate => Ok(FetchedBody {
                    bytes: bytes.slice(..cap),
                    truncated: true,
                }),
            }
        })
    }

    fn open<'a>(
        &'a self,
        _request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<SourceStream, FetchError>> {
        Box::pin(async { Err(FetchError::Rejected) })
    }
}

/// Accepts every container, answers [`WORDS`] and counts its calls.
#[derive(Default)]
pub struct Words {
    calls: AtomicUsize,
}

impl Words {
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Transcriber for Words {
    fn enabled(&self) -> bool {
        true
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
        Box::pin(async {
            Ok(Transcript {
                text: WORDS.into(),
                usage_seconds: None,
            })
        })
    }
}

/// An upload path that transcribes only the first 65 seconds, as a future
/// transformer would for long audio.
pub struct FirstMinute;

impl AudioTranscoder for FirstMinute {
    fn fetch_cap(&self) -> u64 {
        1 << 20
    }
    fn plan<'a>(
        &'a self,
        original: Bytes,
        _sniffed: Sniffed,
        _channel: &'a ChannelId,
        _deadline: Instant,
    ) -> BoxFuture<'a, Result<PlannedUpload, DeriveError>> {
        Box::pin(async move {
            Ok(PlannedUpload {
                input: AudioInput {
                    bytes: original,
                    container: AudioContainer::Mp3,
                },
                metered_seconds: 65,
                truncated_at_seconds: Some(65),
            })
        })
    }
}

/// A transformer whose every transcode reaches its cap: ten minutes of PCM
/// at the default speech-to-text limits.
pub struct TenMinutes;

impl MediaTransformer for TenMinutes {
    fn keyframes(
        &self,
        _request: KeyframeRequest,
    ) -> BoxFuture<'_, Result<Keyframes, TransformError>> {
        Box::pin(async { Err(TransformError::Unavailable) })
    }

    fn audio(&self, request: AudioRequest) -> BoxFuture<'_, Result<Pcm, TransformError>> {
        let data_len = usize::from(request.seconds) * 32_000;
        Box::pin(async move {
            Ok(Pcm {
                wav: Bytes::from_static(&[0; 44]),
                data_len,
                truncated: true,
            })
        })
    }
}

/// The speech-to-text side of a media service.
pub enum Speech {
    Words,
    Disabled,
    FirstMinute,
    /// Every input transcoded by the transformer, and truncated.
    Transformer,
}

pub struct Media {
    pub fixture: Fixture,
    pub source: Arc<Source>,
    pub words: Arc<Words>,
    pub service: Arc<MediaService>,
    pub runner: TurnRunner,
}

impl Media {
    /// A runner preparing with `budget`, whose `cma_codex` bindings are
    /// tool-bearing.
    pub async fn new(speech: Speech, budget: Duration) -> Self {
        let fixture = Fixture::new(FakeProviderConfig::default(), TurnConfig::default(), 8).await;
        let source = Arc::new(Source::default());
        let words = Arc::new(Words::default());
        let stt = SttConfig::default();
        let (transcriber, transcoder): (Arc<dyn Transcriber>, Arc<dyn AudioTranscoder>) =
            match speech {
                Speech::Words => (
                    words.clone(),
                    Arc::new(NoTranscoder::new(&stt, words.clone())),
                ),
                Speech::Disabled => {
                    let disabled = Arc::new(DisabledTranscriber);
                    (
                        disabled.clone(),
                        Arc::new(NoTranscoder::new(&stt, disabled)),
                    )
                }
                Speech::FirstMinute => (words.clone(), Arc::new(FirstMinute)),
                Speech::Transformer => (
                    words.clone(),
                    Arc::new(TransformerTranscoder::new(
                        Arc::new(TenMinutes),
                        &stt,
                        words.clone(),
                        MediaConfig::default().max_bytes,
                    )),
                ),
            };
        let registry = Arc::new(SourceRegistry::new(
            source.clone(),
            source.clone(),
            source.clone(),
            source.clone(),
        ));
        let decode = Arc::new(Semaphore::new(1));
        let tools = Arc::new(MediaToolProviders::from_iter(["cma_codex"
            .parse::<ProviderSlug>()
            .unwrap()]));
        let service = Arc::new(MediaService::new(MediaDeps {
            visuals: Arc::new(PlatformThumbnail::new(registry.clone(), decode.clone())),
            sources: registry,
            transcriber,
            transcoder,
            stt: Arc::new(SttSpend::new(&stt)),
            limits: MediaLimits::from_config(&MediaConfig::default()),
            // The key ring the turn fixture seals its event_refs with.
            keys: Arc::new(
                MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1; 32]))).unwrap(),
            ),
            tool_providers: tools.clone(),
            decode,
            inflight: InflightBytes::new(&MediaLimits::from_config(&MediaConfig::default())),
        }));
        let runner = fixture
            .runner
            .clone()
            .with_media_tool_providers(tools)
            .with_media(Some(service.clone()), config(budget));
        Self {
            fixture,
            source,
            words,
            service,
            runner,
        }
    }

    /// A turn on the fixture channel, rebound by `edit`, carrying `items`.
    pub fn turn(
        &self,
        event: &str,
        items: Vec<Attachment>,
        edit: impl FnOnce(&mut ChannelDefinition),
    ) -> LaunchRequest {
        let mut request = self.fixture.request_with_kind(event, Some(message(items)));
        request.channel = rebound(&self.fixture.channel, edit);
        request
    }

    /// Launch `request` and return the provider input it produced.
    pub async fn input(&self, request: LaunchRequest) -> String {
        let outcome = self.runner.launch(request).await;
        assert!(matches!(outcome, LaunchOutcome::Accepted { .. }));
        last_input(&self.fixture)
    }
}

pub fn config(budget: Duration) -> PrepareConfig {
    PrepareConfig {
        budget,
        ..PrepareConfig::default()
    }
}

/// The input text of the newest provider turn request.
pub fn last_input(fixture: &Fixture) -> String {
    let request = fixture
        .fake
        .requests()
        .into_iter()
        .rev()
        .find(|request| request.method == "POST")
        .expect("a provider turn");
    let body: Value = serde_json::from_str(&request.body).unwrap();
    body["input"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// `channel` with its definition edited, under the same identity.
pub fn rebound(
    channel: &ResolvedChannel,
    edit: impl FnOnce(&mut ChannelDefinition),
) -> Arc<ResolvedChannel> {
    let mut definition = channel.definition.clone();
    edit(&mut definition);
    Arc::new(ResolvedChannel {
        id: channel.id.clone(),
        pointer: channel.pointer.clone(),
        definition,
        agent_key_hash: channel.agent_key_hash,
        loaded_at: channel.loaded_at,
        version: channel.version,
    })
}

pub fn message(attachments: Vec<Attachment>) -> ActivityKind {
    ActivityKind::Message {
        text: "see attached".into(),
        attachments,
        mentions_bot: true,
        reply_to: None,
        album_id: None,
    }
}

/// A relayed attachment at `index` with only the facts a test sets.
pub fn item(index: u8, kind: AttachmentKind, variant: Option<MediaVariant>) -> Attachment {
    Attachment {
        reference: format!("item-{index}"),
        kind,
        filename: None,
        mime_type: None,
        size_bytes: None,
        source: Some(AttachmentSource::NyxidRelay { index }),
        variant,
        duration_seconds: None,
        width: None,
        height: None,
        thumbnail: None,
        emoji: None,
    }
}

/// A voice note at `index` declaring `seconds`, served as a small MP3.
pub fn voice(media: &Media, index: u8, seconds: Option<u32>) -> Attachment {
    media.source.serve(index, Ok(mp3()));
    Attachment {
        mime_type: Some("audio/ogg".into()),
        duration_seconds: seconds,
        ..item(index, AttachmentKind::Audio, Some(MediaVariant::Voice))
    }
}

/// Bytes that sniff as MP3.
pub fn mp3() -> Vec<u8> {
    let mut bytes = b"ID3\x04\0\0\0\0\0\0".to_vec();
    bytes.resize(2_048, 0);
    bytes
}

/// Bytes that sniff as MP4 video.
pub fn mp4() -> Vec<u8> {
    let mut bytes = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom".to_vec();
    bytes.resize(2_048, 0);
    bytes
}
