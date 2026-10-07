//! Audio sets over the scripted job launcher, in paused time: the audio
//! deadline, PCM validation and truncation, the in-flight reservations of
//! every job, and refusals that run no job.
use super::transform::{audio, keyframes, video, Harness, Step, SECOND};
use cmaeg::media::transform::{
    launcher::JobSpec,
    outcome::{StderrClass, TransformOutcome},
    MediaTransformer, NoTransformer, TransformError,
};
use std::time::Duration;
use tokio::time::Instant;

#[tokio::test(start_paused = true)]
async fn audio_set_deadline_is_20s() {
    let harness = Harness::new(|_: &JobSpec| Step::ok(Duration::from_secs(25), vec![0; 64]));
    let started = Instant::now();
    let error = harness
        .transformer
        .audio(audio("a", "voice", 600))
        .await
        .unwrap_err();
    assert_eq!(error, TransformError::job(TransformOutcome::Timeout));
    assert_eq!(harness.ran()[0].deadline, started + Duration::from_secs(20));
    assert_eq!(Instant::now(), started + Duration::from_secs(20));
}

#[tokio::test(start_paused = true)]
async fn odd_or_empty_pcm_is_output_invalid() {
    for pcm in [Vec::new(), vec![1, 2, 3]] {
        let harness = Harness::new(move |_: &JobSpec| Step::ok(SECOND, pcm.clone()));
        let error = harness
            .transformer
            .audio(audio("a", "voice", 5))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            TransformError::Job {
                outcome: TransformOutcome::OutputInvalid,
                stderr_class: StderrClass::None
            }
        );
    }
}

#[tokio::test(start_paused = true)]
async fn audio_truncation_flag() {
    let full = Harness::new(|_: &JobSpec| Step::ok(SECOND, vec![7; 2 * 32_000]));
    let pcm = full
        .transformer
        .audio(audio("a", "voice", 2))
        .await
        .unwrap();
    assert_eq!(pcm.data_len, 64_000);
    assert!(pcm.truncated);
    assert_eq!(pcm.wav.len(), 44 + 64_000);
    assert_eq!(&pcm.wav[..4], b"RIFF");
    assert_eq!(&pcm.wav[40..44], &64_000_u32.to_le_bytes());
    assert_eq!(full.ran()[0].cap, 64_000);
    assert_eq!(full.ran()[0].headroom, 44);
    let short = Harness::new(|_: &JobSpec| Step::ok(SECOND, vec![7; 32_000]));
    let pcm = short
        .transformer
        .audio(audio("a", "voice", 2))
        .await
        .unwrap();
    assert_eq!(pcm.data_len, 32_000);
    assert!(!pcm.truncated);
    // The job's output beyond its cap never reaches the WAV.
    let over = Harness::new(|_: &JobSpec| Step::ok(SECOND, vec![7; 70_000]));
    let pcm = over
        .transformer
        .audio(audio("a", "voice", 2))
        .await
        .unwrap();
    assert_eq!(pcm.data_len, 64_000);
    assert!(pcm.truncated);
}

#[tokio::test(start_paused = true)]
async fn inflight_reserved_before_spawn_and_released_after() {
    let audio_harness = Harness::new(|_: &JobSpec| Step::ok(SECOND, vec![0; 320_000]));
    let total = audio_harness.inflight.available_bytes();
    let pcm = audio_harness
        .transformer
        .audio(audio("a", "voice", 10))
        .await
        .unwrap();
    let reserved = (44_u64 + 320_000).div_ceil(1024) * 1024;
    assert_eq!(audio_harness.ran()[0].inflight_free, total - reserved);
    assert_eq!(audio_harness.inflight.available_bytes(), total);
    assert_eq!(pcm.data_len, 320_000);

    let video_harness = Harness::new(video("10", SECOND));
    video_harness
        .transformer
        .keyframes(keyframes("video", 2))
        .await
        .unwrap();
    let ran = video_harness.ran();
    assert_eq!(ran[0].inflight_free, total - 16_384);
    assert_eq!(ran[1].inflight_free, total - 262_144);
    assert_eq!(ran[2].inflight_free, total - 262_144);
    assert_eq!(video_harness.inflight.available_bytes(), total);
}

#[tokio::test]
async fn no_transformer_refuses_everything() {
    assert_eq!(
        NoTransformer
            .keyframes(keyframes("video", 6))
            .await
            .unwrap_err(),
        TransformError::Unavailable
    );
    assert_eq!(
        NoTransformer
            .audio(audio("a", "voice", 5))
            .await
            .unwrap_err(),
        TransformError::Unavailable
    );
}

#[tokio::test(start_paused = true)]
async fn an_out_of_range_length_is_refused_without_a_job() {
    let harness = Harness::new(|_: &JobSpec| Step::ok(SECOND, vec![0; 2]));
    for seconds in [0, 1_501] {
        assert_eq!(
            harness
                .transformer
                .audio(audio("a", "voice", seconds))
                .await
                .unwrap_err(),
            TransformError::Unavailable
        );
    }
    assert!(harness.ran().is_empty());
}
