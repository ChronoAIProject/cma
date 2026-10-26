//! The media service with the transformer behind its seams, over a fake
//! transformer: the fake and its service, keyframes and the thumbnail
//! fallback. Transcripts are in `transformer_transcript.rs`.
use super::{
    fakes::{attachment, message, target, telegram_file, telegram_source, FakeSource},
    image_fixtures as fixtures,
    service::{failed, limits, ready, ring, LONG},
};
use bytes::Bytes;
use cmaeg::{
    media::{
        budget::InflightBytes,
        derive::{Artifact, DeriveError, Derived},
        image::PREVIEW,
        service::{MediaCapabilities, MediaDeps, MediaService},
        transcribe::{
            meter::SttSpend, AudioContainer, AudioInput, TranscribeError, Transcriber, Transcript,
        },
        transform::{
            adapters::{TransformerKeyframes, TransformerTranscoder},
            outcome::{StderrClass, TransformOutcome},
            AudioRequest, KeyframeRequest, Keyframes, MediaTransformer, Pcm, TransformError,
        },
        visual::VisualKind,
    },
    MediaToolProviders, SttConfig,
};
use cmaeg_core::{activity::AttachmentKind, ids::ChannelId};
use futures_util::future::BoxFuture;
use std::sync::{Arc, Mutex};
use tokio::{sync::Semaphore, time::Instant};

type KeyframeScript = dyn Fn() -> Result<Keyframes, TransformError> + Send + Sync;
type AudioScript = dyn Fn(&AudioRequest) -> Result<Pcm, TransformError> + Send + Sync;

/// Answers from scripts and records what it was asked and the pod's free
/// in-flight bytes during each call.
pub(super) struct FakeTransformer {
    keyframes: Box<KeyframeScript>,
    audio: Box<AudioScript>,
    inflight: InflightBytes,
    pub seen: Mutex<Vec<(usize, u64)>>,
}

impl MediaTransformer for FakeTransformer {
    fn keyframes(
        &self,
        request: KeyframeRequest,
    ) -> BoxFuture<'_, Result<Keyframes, TransformError>> {
        assert_eq!(request.frames, 6);
        self.seen
            .lock()
            .unwrap()
            .push((request.input.len(), self.inflight.available_bytes()));
        Box::pin(async move { (self.keyframes)() })
    }

    fn audio(&self, request: AudioRequest) -> BoxFuture<'_, Result<Pcm, TransformError>> {
        self.seen
            .lock()
            .unwrap()
            .push((request.input.len(), self.inflight.available_bytes()));
        let result = (self.audio)(&request);
        Box::pin(async move { result })
    }
}

/// Accepts the containers it is given and records each upload's container.
pub(super) struct Recording {
    accepts: Vec<AudioContainer>,
    pub uploads: Mutex<Vec<AudioContainer>>,
}

impl Transcriber for Recording {
    fn enabled(&self) -> bool {
        true
    }

    fn accepts(&self, container: AudioContainer) -> bool {
        self.accepts.contains(&container)
    }

    fn transcribe<'a>(
        &'a self,
        input: AudioInput,
        _deadline: Instant,
    ) -> BoxFuture<'a, Result<Transcript, TranscribeError>> {
        self.uploads.lock().unwrap().push(input.container);
        Box::pin(async {
            Ok(Transcript {
                text: "hello".into(),
                usage_seconds: None,
            })
        })
    }
}

pub(super) struct Built {
    pub service: MediaService,
    pub transformer: Arc<FakeTransformer>,
    pub transcriber: Arc<Recording>,
    pub stt: Arc<SttSpend>,
    pub inflight: InflightBytes,
}

pub(super) fn build(
    source: &Arc<FakeSource>,
    stt_config: SttConfig,
    accepts: &[AudioContainer],
    keyframes: impl Fn() -> Result<Keyframes, TransformError> + Send + Sync + 'static,
    audio: impl Fn(&AudioRequest) -> Result<Pcm, TransformError> + Send + Sync + 'static,
) -> Built {
    let limits = limits();
    let inflight = InflightBytes::new(&limits);
    let transformer = Arc::new(FakeTransformer {
        keyframes: Box::new(keyframes),
        audio: Box::new(audio),
        inflight: inflight.clone(),
        seen: Mutex::default(),
    });
    let transcriber = Arc::new(Recording {
        accepts: accepts.to_vec(),
        uploads: Mutex::default(),
    });
    let stt = Arc::new(SttSpend::new(&stt_config));
    let registry = source.registry();
    let decode = Arc::new(Semaphore::new(1));
    let service = MediaService::new(MediaDeps {
        visuals: Arc::new(TransformerKeyframes::new(
            transformer.clone(),
            registry.clone(),
            decode.clone(),
            limits.max_bytes,
            6,
        )),
        sources: registry,
        transcoder: Arc::new(TransformerTranscoder::new(
            transformer.clone(),
            &stt_config,
            transcriber.clone(),
            limits.max_bytes,
        )),
        transcriber: transcriber.clone(),
        stt: stt.clone(),
        limits,
        keys: Arc::new(ring()),
        tool_providers: Arc::new(MediaToolProviders::default()),
        decode,
        inflight: inflight.clone(),
    });
    Built {
        service,
        transformer,
        transcriber,
        stt,
        inflight,
    }
}

pub(super) const ALL: [AudioContainer; 3] = [
    AudioContainer::Mp3,
    AudioContainer::Ogg,
    AudioContainer::Wav,
];

pub(super) fn no_frames() -> Result<Keyframes, TransformError> {
    Err(TransformError::NoFrames)
}

fn no_audio(_: &AudioRequest) -> Result<Pcm, TransformError> {
    Err(TransformError::Unavailable)
}

/// Six distinct large frames, as a keyframe set returns them.
fn six_frames() -> Result<Keyframes, TransformError> {
    let frames = (0..6_u8)
        .map(|index| {
            let jpeg = fixtures::jpeg(&fixtures::rgb(1280, 720, 0), 90);
            (Bytes::from(jpeg), 500 + u64::from(index) * 1_000)
        })
        .collect();
    Ok(Keyframes {
        frames,
        duration_ms: Some(6_000),
        partial: false,
    })
}

/// `seconds` of PCM in a WAV, `truncated` as given.
pub(super) fn pcm(seconds: usize, truncated: bool) -> Result<Pcm, TransformError> {
    let data_len = seconds * 32_000;
    Ok(Pcm {
        wav: Bytes::from(vec![0; 44 + data_len.min(64)]),
        data_len,
        truncated,
    })
}

fn video_target(source: &FakeSource, thumbnail: bool) -> cmaeg::media::target::MediaTarget {
    source.serve(&telegram_file("video"), Ok(b"video bytes".to_vec()));
    source.serve(
        &telegram_file("thumb"),
        Ok(fixtures::jpeg(&fixtures::rgb(320, 180, 8), 90)),
    );
    let mut video = attachment(AttachmentKind::Video, Some(telegram_file("video")));
    if thumbnail {
        video.thumbnail = Some(telegram_file("thumb"));
    }
    target(telegram_source(), message(vec![video]))
}

pub(super) fn channel(target: &cmaeg::media::target::MediaTarget) -> ChannelId {
    target.channel.id.clone()
}

#[tokio::test]
async fn frames_are_at_most_six_keyframes_within_a_megabyte() {
    let source = FakeSource::new();
    let target = video_target(&source, true);
    let built = build(&source, SttConfig::default(), &ALL, six_frames, no_audio);
    assert_eq!(
        built.service.capabilities(),
        MediaCapabilities {
            transcriber_enabled: true,
            visuals: VisualKind::Keyframes,
        }
    );
    let derived = ready(
        built
            .service
            .derive(&target, 0, Artifact::Visuals, LONG)
            .await,
    );
    let Derived::Visuals(frames) = &*derived else {
        panic!("not visuals: {derived:?}");
    };
    assert_eq!(frames.len(), 6);
    let positions: Vec<_> = frames.iter().map(|frame| frame.at_ms).collect();
    assert_eq!(
        positions,
        [500, 1_500, 2_500, 3_500, 4_500, 5_500].map(Some)
    );
    assert!(frames
        .iter()
        .all(|frame| frame.width.max(frame.height) <= PREVIEW.max_side));
    let total: usize = frames.iter().map(|frame| frame.bytes.len()).sum();
    assert!(total <= 1 << 20, "{total} bytes");
    let seen = source.seen();
    assert_eq!(seen.len(), 1, "the original only, not the thumbnail");
    assert_eq!(seen[0].cap, limits().max_bytes);
    assert_eq!(built.inflight.available_bytes(), limits().inflight_bytes);
}

#[tokio::test]
async fn thumbnail_fallback_reports_platform_thumbnail_origin() {
    let source = FakeSource::new();
    let target = video_target(&source, true);
    let failing = || {
        Err(TransformError::Job {
            outcome: TransformOutcome::ResourceLimit,
            stderr_class: StderrClass::None,
        })
    };
    let built = build(&source, SttConfig::default(), &ALL, failing, no_audio);
    let derived = ready(
        built
            .service
            .derive(&target, 0, Artifact::Visuals, LONG)
            .await,
    );
    let Derived::Visuals(frames) = &*derived else {
        panic!("not visuals: {derived:?}");
    };
    assert_eq!(frames.len(), 1);
    // No position: reported as the platform's thumbnail, not a keyframe.
    assert_eq!(frames[0].at_ms, None);
    assert_eq!(built.transformer.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn failed_keyframes_without_a_thumbnail_report_the_mapped_reason() {
    let cases = [
        (TransformError::NoFrames, DeriveError::TimedOut),
        (
            TransformError::job(TransformOutcome::SandboxUnavailable),
            DeriveError::TransformFailed,
        ),
        (
            TransformError::Job {
                outcome: TransformOutcome::ExitNonzero,
                stderr_class: StderrClass::UnsupportedCodec,
            },
            DeriveError::UnsupportedFormat,
        ),
    ];
    for (error, expected) in cases {
        let source = FakeSource::new();
        let target = video_target(&source, false);
        let built = build(
            &source,
            SttConfig::default(),
            &ALL,
            move || Err(error),
            no_audio,
        );
        assert_eq!(
            failed(
                built
                    .service
                    .derive(&target, 0, Artifact::Visuals, LONG)
                    .await
            ),
            expected
        );
    }
    // A failed fallback keeps the keyframe reason.
    let source = FakeSource::new();
    let target = video_target(&source, true);
    source.serve(&telegram_file("thumb"), Ok(b"not an image".to_vec()));
    let built = build(&source, SttConfig::default(), &ALL, no_frames, no_audio);
    assert_eq!(
        failed(
            built
                .service
                .derive(&target, 0, Artifact::Visuals, LONG)
                .await
        ),
        DeriveError::TimedOut
    );
}
