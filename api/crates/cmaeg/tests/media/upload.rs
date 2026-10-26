//! The audio-transcoding seam: the direct path's refusals and metering, and
//! a replacement upload path plugged in without changing any caller.
use super::{
    fakes::{attachment, message, target_on, url, FakeSource},
    service::{failed, limits, mp3_of, ready, service_from, LONG},
    stt_meter::{sample, tight},
};
use bytes::Bytes;
use cmaeg::{
    media::{
        derive::{Artifact, DeriveError, Derived},
        sniff::{sniff, Sniffed},
        target::MediaTarget,
        transcribe::{
            meter::{direct_seconds, SpendRefusal, SttSpend},
            upload::{AudioTranscoder, NoTranscoder, PlannedUpload},
            AudioContainer, AudioInput, DisabledTranscriber, TranscribeError, Transcriber,
            Transcript,
        },
    },
    SttConfig,
};
use cmaeg_core::{
    activity::{Attachment, AttachmentKind, SourceKind},
    ids::ChannelId,
};
use futures_util::future::BoxFuture;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::time::Instant;

/// Accepts every container, answers with scripted usage and records what it
/// was sent.
#[derive(Default)]
pub(super) struct ScriptedTranscriber {
    usage: Option<u32>,
    calls: AtomicUsize,
    containers: Mutex<Vec<AudioContainer>>,
}

impl ScriptedTranscriber {
    pub(super) fn reporting(usage: Option<u32>) -> Arc<Self> {
        Arc::new(Self {
            usage,
            ..Self::default()
        })
    }

    pub(super) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub(super) fn containers(&self) -> Vec<AudioContainer> {
        self.containers.lock().unwrap().clone()
    }
}

impl Transcriber for ScriptedTranscriber {
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
        self.containers.lock().unwrap().push(input.container);
        Box::pin(async move {
            Ok(Transcript {
                text: "scripted words".into(),
                usage_seconds: self.usage,
            })
        })
    }
}

/// An upload path that re-encodes to a fixed plan and records the channels
/// it planned for.
struct FakeTranscoder {
    fetch_cap: u64,
    container: AudioContainer,
    metered_seconds: u32,
    truncated_at_seconds: Option<u32>,
    channels: Mutex<Vec<ChannelId>>,
}

impl FakeTranscoder {
    fn new(fetch_cap: u64, container: AudioContainer, metered_seconds: u32) -> Self {
        Self {
            fetch_cap,
            container,
            metered_seconds,
            truncated_at_seconds: None,
            channels: Mutex::default(),
        }
    }
}

impl AudioTranscoder for FakeTranscoder {
    fn fetch_cap(&self) -> u64 {
        self.fetch_cap
    }

    fn plan<'a>(
        &'a self,
        original: Bytes,
        _sniffed: Sniffed,
        channel: &'a ChannelId,
        _deadline: Instant,
    ) -> BoxFuture<'a, Result<PlannedUpload, DeriveError>> {
        self.channels.lock().unwrap().push(channel.clone());
        Box::pin(async move {
            Ok(PlannedUpload {
                input: AudioInput {
                    bytes: original,
                    container: self.container,
                },
                metered_seconds: self.metered_seconds,
                truncated_at_seconds: self.truncated_at_seconds,
            })
        })
    }
}

/// A custom message on `channel` carrying one MP3 note of `length` bytes.
pub(super) fn voice(
    source: &FakeSource,
    channel: &str,
    name: &str,
    length: usize,
    edit: impl FnOnce(&mut Attachment),
) -> MediaTarget {
    audio_note(source, channel, name, mp3_of(length), edit)
}

/// A custom message on `channel` carrying one audio attachment, served as
/// `body`.
pub(super) fn audio_note(
    source: &FakeSource,
    channel: &str,
    name: &str,
    body: Vec<u8>,
    edit: impl FnOnce(&mut Attachment),
) -> MediaTarget {
    source.serve(&url(name), Ok(body));
    let mut item = attachment(AttachmentKind::Audio, Some(url(name)));
    edit(&mut item);
    target_on(channel, SourceKind::Custom, message(vec![item]))
}

async fn plan(
    transcoder: &dyn AudioTranscoder,
    bytes: Vec<u8>,
) -> Result<PlannedUpload, DeriveError> {
    let sniffed = sniff(&bytes);
    let channel = ChannelId::new("cmaeg1.ch.stt-plan").unwrap();
    let deadline = Instant::now() + LONG;
    transcoder
        .plan(Bytes::from(bytes), sniffed, &channel, deadline)
        .await
}

fn direct(transcriber: Arc<dyn Transcriber>) -> NoTranscoder {
    NoTranscoder::new(&SttConfig::default(), transcriber)
}

#[tokio::test]
async fn the_direct_path_refuses_an_original_over_its_cap_as_too_long() {
    let transcoder = direct(ScriptedTranscriber::reporting(None));
    assert_eq!(transcoder.fetch_cap(), 2_400_000);
    assert_eq!(
        plan(&transcoder, mp3_of(2_400_001)).await.unwrap_err(),
        DeriveError::TooLong
    );
    let smaller = NoTranscoder::new(
        &SttConfig {
            max_bytes: 1_048_576,
            ..SttConfig::default()
        },
        ScriptedTranscriber::reporting(None),
    );
    assert_eq!(smaller.fetch_cap(), 1_048_576);
}

#[tokio::test]
async fn the_direct_path_refuses_containers_its_transcriber_cannot_take() {
    let mut amr = b"#!AMR\n".to_vec();
    amr.resize(4096, 0);
    assert_eq!(sniff(&amr).mime, "audio/amr");
    let transcoder = direct(ScriptedTranscriber::reporting(None));
    assert_eq!(
        plan(&transcoder, amr).await.unwrap_err(),
        DeriveError::UnsupportedFormat
    );
    let refusing = direct(Arc::new(DisabledTranscriber));
    assert_eq!(
        plan(&refusing, mp3_of(4096)).await.unwrap_err(),
        DeriveError::UnsupportedFormat
    );
}

#[tokio::test]
async fn the_direct_path_uploads_the_original_metered_by_its_size() {
    let transcoder = direct(ScriptedTranscriber::reporting(None));
    for length in [10_000, 2_400_000] {
        let planned = plan(&transcoder, mp3_of(length)).await.unwrap();
        assert_eq!(planned.metered_seconds, direct_seconds(length as u64));
        assert_eq!(planned.truncated_at_seconds, None);
        assert_eq!(planned.input.container, AudioContainer::Mp3);
        assert_eq!(planned.input.bytes.len(), length);
    }
}

#[tokio::test]
async fn a_truncating_transcoder_marks_the_transcript_and_sees_the_channel() {
    let source = FakeSource::new();
    let target = voice(&source, "stt-truncate", "long-note", 10_000, |_| {});
    let transcriber = ScriptedTranscriber::reporting(None);
    let transcoder = Arc::new(FakeTranscoder {
        truncated_at_seconds: Some(600),
        ..FakeTranscoder::new(2_400_000, AudioContainer::Wav, 600)
    });
    let service = service_from(
        &source,
        limits(),
        transcriber,
        Arc::new(SttSpend::new(&SttConfig::default())),
        transcoder.clone(),
    );
    let derived = ready(service.derive(&target, 0, Artifact::Transcript, LONG).await);
    let Derived::Transcript {
        truncated_at_seconds,
        document,
        ..
    } = &*derived
    else {
        panic!("{derived:?}");
    };
    assert_eq!(*truncated_at_seconds, Some(600));
    assert_eq!(document.pages, ["scripted words"]);
    let planned_for = transcoder.channels.lock().unwrap().clone();
    assert_eq!(planned_for, std::slice::from_ref(&target.channel.id));
}

#[tokio::test]
async fn a_transcoder_with_a_larger_cap_is_fetched_and_charged_its_own_plan() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let source = FakeSource::new();
    let target = voice(&source, "stt-transcode", "big-note", 5_000_000, |_| {});
    let transcriber = ScriptedTranscriber::reporting(None);
    let stt = Arc::new(SttSpend::new(&tight(600)));
    let service = service_from(
        &source,
        limits(),
        transcriber.clone(),
        stt.clone(),
        Arc::new(FakeTranscoder::new(
            20 * 1024 * 1024,
            AudioContainer::Wav,
            42,
        )),
    );
    ready(service.derive(&target, 0, Artifact::Transcript, LONG).await);
    assert_eq!(source.seen()[0].cap, 20 * 1024 * 1024);
    assert_eq!(transcriber.containers(), [AudioContainer::Wav]);
    assert_eq!(
        sample(&handle.render(), "cmaeg_stt_audio_seconds_total"),
        Some(42.0)
    );
    // 42 s left both budgets: the channel and the pod each hold 558 s.
    let channel = &target.channel.id;
    assert_eq!(stt.charge(channel, 559), Err(SpendRefusal::ChannelBudget));
    let other = ChannelId::new("cmaeg1.ch.stt-other").unwrap();
    assert_eq!(stt.charge(&other, 559), Err(SpendRefusal::GlobalBudget));
    // A global refusal leaves the other channel charged, so probe a third.
    let third = ChannelId::new("cmaeg1.ch.stt-third").unwrap();
    assert_eq!(stt.charge(&third, 558), Ok(()));
    // The direct path would have refused the same original.
    let direct_service = service_from(
        &source,
        limits(),
        transcriber.clone(),
        Arc::new(SttSpend::new(&SttConfig::default())),
        Arc::new(direct(transcriber.clone())),
    );
    assert_eq!(
        failed(
            direct_service
                .derive(&target, 0, Artifact::Transcript, LONG)
                .await
        ),
        DeriveError::TooLong
    );
    assert_eq!(transcriber.calls(), 1);
}
