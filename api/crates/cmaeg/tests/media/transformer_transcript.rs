//! Transcripts with the transformer behind the media seams, over the fake
//! transformer: transcoded and truncated transcripts, exact metering, the
//! direct fallback, spend refusals and the in-flight budget.
use super::{
    fakes::{attachment, message, url, FakeSource},
    service::{failed, files, mp3_of, ready, LONG},
    stt_fake::ogg,
    transformer_service::{build, channel, no_frames, pcm, ALL},
};
use cmaeg::{
    media::{
        derive::{Artifact, DeriveError, Derived},
        transcribe::AudioContainer,
        transform::{
            adapters::transcode_seconds, outcome::TransformOutcome, AudioRequest, TransformError,
        },
    },
    SttConfig,
};
use cmaeg_core::activity::AttachmentKind;

#[tokio::test]
async fn ogg_is_transcoded_and_metered_by_its_pcm_seconds() {
    let source = FakeSource::new();
    let voice = files(&source, &[("voice.oga", ogg(4_096))]);
    let stt_config = SttConfig {
        channel_seconds_per_hour: 600,
        seconds_per_hour: 600,
        ..SttConfig::default()
    };
    let built = build(&source, stt_config, &ALL, no_frames, |request| {
        assert_eq!(request.seconds, 600);
        pcm(10, false)
    });
    let derived = ready(
        built
            .service
            .derive(&voice, 0, Artifact::Transcript, LONG)
            .await,
    );
    let Derived::Transcript {
        truncated_at_seconds,
        ..
    } = &*derived
    else {
        panic!("not a transcript: {derived:?}");
    };
    assert_eq!(*truncated_at_seconds, None);
    assert_eq!(
        *built.transcriber.uploads.lock().unwrap(),
        [AudioContainer::Wav]
    );
    // Exactly the ten PCM seconds were charged: 590 remain this hour.
    let channel = channel(&voice);
    assert!(built.stt.charge(&channel, 591).is_err());
    assert!(built.stt.charge(&channel, 590).is_ok());
}

#[tokio::test]
async fn long_audio_is_truncated_with_the_notice() {
    let source = FakeSource::new();
    let voice = files(&source, &[("lecture.oga", ogg(8_192))]);
    let built = build(&source, SttConfig::default(), &ALL, no_frames, |_| {
        pcm(600, true)
    });
    let derived = ready(
        built
            .service
            .derive(&voice, 0, Artifact::Transcript, LONG)
            .await,
    );
    let Derived::Transcript {
        truncated_at_seconds,
        ..
    } = &*derived
    else {
        panic!("not a transcript: {derived:?}");
    };
    assert_eq!(*truncated_at_seconds, Some(600));
    assert_eq!(transcode_seconds(&SttConfig::default()), 600);
    let small = SttConfig {
        max_bytes: 1_048_576,
        ..SttConfig::default()
    };
    assert_eq!(transcode_seconds(&small), 32);
}

#[tokio::test]
async fn a_failed_transcode_falls_back_to_the_direct_path_only_when_eligible() {
    let source = FakeSource::new();
    let target = files(
        &source,
        &[("note.mp3", mp3_of(8_000)), ("note.oga", ogg(8_000))],
    );
    let failing = |_: &AudioRequest| Err(TransformError::job(TransformOutcome::ExitNonzero));
    // The endpoint takes MP3 and WAV but not Ogg.
    let accepts = [AudioContainer::Mp3, AudioContainer::Wav];
    let built = build(&source, SttConfig::default(), &accepts, no_frames, failing);
    ready(
        built
            .service
            .derive(&target, 0, Artifact::Transcript, LONG)
            .await,
    );
    assert_eq!(
        *built.transcriber.uploads.lock().unwrap(),
        [AudioContainer::Mp3]
    );
    assert_eq!(
        failed(
            built
                .service
                .derive(&target, 1, Artifact::Transcript, LONG)
                .await
        ),
        DeriveError::DecodeFailed
    );
    assert_eq!(built.transcriber.uploads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn an_endpoint_without_wav_keeps_the_direct_path() {
    let source = FakeSource::new();
    let target = files(&source, &[("note.mp3", mp3_of(8_000))]);
    let built = build(
        &source,
        SttConfig::default(),
        &[AudioContainer::Mp3],
        no_frames,
        |_| panic!("no transcode for an endpoint without WAV"),
    );
    ready(
        built
            .service
            .derive(&target, 0, Artifact::Transcript, LONG)
            .await,
    );
    assert_eq!(
        *built.transcriber.uploads.lock().unwrap(),
        [AudioContainer::Mp3]
    );
}

#[tokio::test]
async fn channel_and_global_budget_refusals_are_rate_limited() {
    let stt_config = SttConfig {
        channel_seconds_per_hour: 600,
        seconds_per_hour: 600,
        ..SttConfig::default()
    };
    let source = FakeSource::new();
    let first = files(&source, &[("one.oga", ogg(4_096))]);
    let built = build(&source, stt_config.clone(), &ALL, no_frames, |_| {
        pcm(601, false)
    });
    assert_eq!(
        failed(
            built
                .service
                .derive(&first, 0, Artifact::Transcript, LONG)
                .await
        ),
        DeriveError::RateLimited
    );
    let source = FakeSource::new();
    let built = build(&source, stt_config, &ALL, no_frames, |_| pcm(400, false));
    let a = super::fakes::target_on(
        "channel-a",
        cmaeg_core::activity::SourceKind::Custom,
        message(vec![attachment(AttachmentKind::Audio, Some(url("a.oga")))]),
    );
    let b = super::fakes::target_on(
        "channel-b",
        cmaeg_core::activity::SourceKind::Custom,
        message(vec![attachment(AttachmentKind::Audio, Some(url("b.oga")))]),
    );
    source.serve(&url("a.oga"), Ok(ogg(4_096)));
    source.serve(&url("b.oga"), Ok(ogg(4_096)));
    ready(
        built
            .service
            .derive(&a, 0, Artifact::Transcript, LONG)
            .await,
    );
    // Channel b's own hour has room; the global hour does not.
    assert_eq!(
        failed(
            built
                .service
                .derive(&b, 0, Artifact::Transcript, LONG)
                .await
        ),
        DeriveError::RateLimited
    );
    assert_eq!(built.transcriber.uploads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn inflight_bytes_are_held_during_audio_and_released_after() {
    let source = FakeSource::new();
    let voice = files(&source, &[("voice.oga", ogg(300_000))]);
    let built = build(&source, SttConfig::default(), &ALL, no_frames, |_| {
        pcm(5, false)
    });
    let total = built.inflight.available_bytes();
    ready(
        built
            .service
            .derive(&voice, 0, Artifact::Transcript, LONG)
            .await,
    );
    let seen = built.transformer.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    let (input, free) = seen[0];
    assert_eq!(input, 300_000);
    assert!(
        free <= total - 300_000,
        "{free} of {total} free during the job"
    );
    assert_eq!(built.inflight.available_bytes(), total);
}
