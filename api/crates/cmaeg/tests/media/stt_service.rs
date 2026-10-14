//! Transcripts through the media service on the direct upload path: size
//! metering, the upload cap, the call rate, the per-channel budget checked
//! before the global backstop, and usage reconciliation.
use super::{
    fakes::FakeSource,
    service::{failed, limits, ready, service_from, LONG},
    stt_meter::{sample, tight},
    upload::{voice, ScriptedTranscriber},
};
use cmaeg::{
    media::{
        derive::{Artifact, DeriveError},
        service::MediaService,
        transcribe::{meter::SttSpend, upload::NoTranscoder},
    },
    SttConfig,
};
use metrics_exporter_prometheus::{PrometheusHandle, PrometheusRecorder};
use std::sync::Arc;

const AUDIO_SECONDS: &str = "cmaeg_stt_audio_seconds_total";
const FULL: usize = 2_400_000;

/// A recorder local to this test's thread, so no other test's calls count.
fn recorder() -> (PrometheusRecorder, PrometheusHandle) {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    (recorder, handle)
}

fn direct_service(
    source: &Arc<FakeSource>,
    stt: &SttConfig,
    transcriber: &Arc<ScriptedTranscriber>,
) -> MediaService {
    service_from(
        source,
        limits(),
        transcriber.clone(),
        Arc::new(SttSpend::new(stt)),
        Arc::new(NoTranscoder::new(stt, transcriber.clone())),
    )
}

/// The count of custom audio items refused with `outcome`.
fn refused(text: &str, outcome: &str) -> Option<f64> {
    let name = format!(
        "cmaeg_media_items_total{{source=\"custom\",kind=\"audio\",outcome=\"{outcome}\"}}"
    );
    sample(text, &name)
}

#[tokio::test]
async fn a_full_direct_upload_is_metered_600_seconds_and_allowed() {
    let (recorder, handle) = recorder();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let source = FakeSource::new();
    let transcriber = ScriptedTranscriber::reporting(None);
    let service = direct_service(&source, &SttConfig::default(), &transcriber);
    let target = voice(&source, "stt-a", "full", FULL, |_| {});
    ready(service.derive(&target, 0, Artifact::Transcript, LONG).await);
    assert_eq!(transcriber.calls(), 1);
    assert_eq!(sample(&handle.render(), AUDIO_SECONDS), Some(600.0));
    assert_eq!(source.seen()[0].cap, 2_400_000);
}

#[tokio::test]
async fn one_byte_over_the_direct_cap_is_too_long_without_a_call() {
    let source = FakeSource::new();
    let transcriber = ScriptedTranscriber::reporting(None);
    let service = direct_service(&source, &SttConfig::default(), &transcriber);
    let target = voice(&source, "stt-a", "over", FULL + 1, |_| {});
    assert_eq!(
        failed(service.derive(&target, 0, Artifact::Transcript, LONG).await),
        DeriveError::TooLong
    );
    assert_eq!(source.calls(), 1);
    assert_eq!(transcriber.calls(), 0);
}

#[tokio::test]
async fn a_declared_size_over_the_cap_is_too_long_without_a_fetch() {
    let source = FakeSource::new();
    let transcriber = ScriptedTranscriber::reporting(None);
    let service = direct_service(&source, &SttConfig::default(), &transcriber);
    let target = voice(&source, "stt-a", "declared", 1_000, |item| {
        item.size_bytes = Some(3_000_000);
    });
    assert_eq!(
        failed(service.derive(&target, 0, Artifact::Transcript, LONG).await),
        DeriveError::TooLong
    );
    assert_eq!(source.calls(), 0);
    assert_eq!(transcriber.calls(), 0);
}

#[tokio::test]
async fn a_lower_media_byte_cap_also_bounds_the_transcript_fetch() {
    let source = FakeSource::new();
    let transcriber = ScriptedTranscriber::reporting(None);
    let stt = SttConfig::default();
    let service = service_from(
        &source,
        cmaeg::media::MediaLimits {
            max_bytes: 1_000_000,
            ..limits()
        },
        transcriber.clone(),
        Arc::new(SttSpend::new(&stt)),
        Arc::new(NoTranscoder::new(&stt, transcriber.clone())),
    );
    let target = voice(&source, "stt-a", "media-cap", 1_000_001, |_| {});
    assert_eq!(
        failed(service.derive(&target, 0, Artifact::Transcript, LONG).await),
        DeriveError::TooLong
    );
    assert_eq!(source.seen()[0].cap, 1_000_000);
    assert_eq!(transcriber.calls(), 0);
}

#[tokio::test]
async fn a_declared_duration_never_lowers_the_charge() {
    let (recorder, handle) = recorder();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let source = FakeSource::new();
    let transcriber = ScriptedTranscriber::reporting(None);
    let service = direct_service(&source, &SttConfig::default(), &transcriber);
    let target = voice(&source, "stt-a", "claims-one-second", FULL, |item| {
        item.duration_seconds = Some(1);
    });
    ready(service.derive(&target, 0, Artifact::Transcript, LONG).await);
    assert_eq!(sample(&handle.render(), AUDIO_SECONDS), Some(600.0));
}

#[tokio::test]
async fn the_channel_budget_is_checked_before_the_global_backstop() {
    let (recorder, handle) = recorder();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let source = FakeSource::new();
    let transcriber = ScriptedTranscriber::reporting(None);
    let service = direct_service(&source, &tight(1_200), &transcriber);

    let first = voice(&source, "stt-a", "a-long", FULL, |_| {});
    ready(service.derive(&first, 0, Artifact::Transcript, LONG).await);
    // Ten metered seconds that channel A no longer has.
    let second = voice(&source, "stt-a", "a-short", 40_000, |_| {});
    assert_eq!(
        failed(service.derive(&second, 0, Artifact::Transcript, LONG).await),
        DeriveError::RateLimited
    );
    assert_eq!(transcriber.calls(), 1);
    assert_eq!(
        refused(&handle.render(), "stt_channel_budget_exhausted"),
        Some(1.0)
    );

    // Channel A's refusal took nothing from the global budget.
    let other = voice(&source, "stt-b", "b-long", FULL, |_| {});
    ready(service.derive(&other, 0, Artifact::Transcript, LONG).await);
    assert_eq!(transcriber.calls(), 2);

    let third = voice(&source, "stt-c", "c-note", 400_000, |_| {});
    assert_eq!(
        failed(service.derive(&third, 0, Artifact::Transcript, LONG).await),
        DeriveError::RateLimited
    );
    assert_eq!(transcriber.calls(), 2);
    let text = handle.render();
    assert_eq!(refused(&text, "stt_budget_exhausted"), Some(1.0));
    assert_eq!(sample(&text, "cmaeg_stt_channels_at_budget"), Some(1.0));
    assert_eq!(sample(&text, AUDIO_SECONDS), Some(1_200.0));
}

#[tokio::test]
async fn reported_usage_above_the_metered_seconds_is_counted() {
    let (recorder, handle) = recorder();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let source = FakeSource::new();
    let transcriber = ScriptedTranscriber::reporting(Some(700));
    let service = direct_service(&source, &SttConfig::default(), &transcriber);
    let target = voice(&source, "stt-a", "low-bitrate", FULL, |_| {});
    ready(service.derive(&target, 0, Artifact::Transcript, LONG).await);
    assert_eq!(sample(&handle.render(), AUDIO_SECONDS), Some(700.0));
}

#[tokio::test]
async fn the_call_rate_refuses_a_second_call_within_the_minute() {
    let (recorder, handle) = recorder();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let source = FakeSource::new();
    let transcriber = ScriptedTranscriber::reporting(None);
    let stt = SttConfig {
        rate_per_minute: 1,
        ..SttConfig::default()
    };
    let service = direct_service(&source, &stt, &transcriber);
    let first = voice(&source, "stt-a", "first", 8_000, |_| {});
    ready(service.derive(&first, 0, Artifact::Transcript, LONG).await);
    let second = voice(&source, "stt-a", "second", 8_000, |_| {});
    assert_eq!(
        failed(service.derive(&second, 0, Artifact::Transcript, LONG).await),
        DeriveError::RateLimited
    );
    assert_eq!(transcriber.calls(), 1);
    assert_eq!(source.calls(), 1, "a refused call must not fetch");
    let text = handle.render();
    assert_eq!(refused(&text, "stt_rate_limited"), Some(1.0));
    assert_eq!(sample(&text, AUDIO_SECONDS), Some(2.0));
}
