//! The OpenAI-compatible transcriber against a fake endpoint: the multipart
//! request, responses, retries, bounds, concurrency, and the end-to-end path
//! through the media service.
use super::{
    fakes::FakeSource,
    service::{failed, limits, mp3_of, ready, service_from, LONG},
    stt_fake::{input, ogg, start, transcriber, FakeStt, Reply, KEY, SPOKEN},
    stt_meter::sample,
    support::{assert_absent, capture_logs, TRACE_ENV},
    upload::audio_note,
};
use cmaeg::{
    media::{
        derive::{Artifact, DeriveError, Derived},
        service::MediaService,
        sniff::sniff,
        transcribe::{
            meter::SttSpend, openai::MAX_CONCURRENT_CALLS, upload::NoTranscoder, AudioContainer,
            TranscribeError, Transcriber,
        },
    },
    telemetry::channel_hash,
};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

const WITH_OGG: (&str, &str) = ("CMAEG_STT_FORMATS", "mp3,mp4,mpeg,mpga,m4a,wav,webm,ogg");

fn far() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

/// A media service whose transcripts go through the real client to `url`,
/// under the spend limits `extra` sets.
fn service(source: &Arc<FakeSource>, url: &str, extra: &[(&'static str, &str)]) -> MediaService {
    let client = transcriber(url, extra);
    let config = super::stt_fake::config_for(url, extra).unwrap();
    service_from(
        source,
        limits(),
        client.clone(),
        Arc::new(SttSpend::new(&config)),
        Arc::new(NoTranscoder::new(&config, client)),
    )
}

/// Wait, in real time, until the endpoint has received `count` requests.
async fn until_received(fake: &FakeStt, count: usize) {
    for _ in 0..500 {
        if fake.requests() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "the endpoint received {} of {count} requests",
        fake.requests()
    );
}

#[tokio::test]
async fn the_request_carries_the_file_model_format_and_platform_key() {
    let (url, fake) = start();
    let client = transcriber(&url, &[WITH_OGG]);
    let audio = ogg(9_000);
    let transcript = client
        .transcribe(input(AudioContainer::Ogg, audio.clone()), far())
        .await
        .unwrap();
    assert_eq!(transcript.text, SPOKEN);
    let [request] = &fake.received()[..] else {
        panic!("expected one request");
    };
    assert_eq!(
        request.authorization.as_deref(),
        Some(&*format!("Bearer {KEY}"))
    );
    assert_eq!(request.content_length, Some(request.body.len() as u64));
    assert_eq!(request.text("model").as_deref(), Some("gpt-transcribe"));
    assert_eq!(request.text("response_format").as_deref(), Some("json"));
    let parts = request.parts();
    let file = parts.iter().find(|part| part.name == "file").unwrap();
    assert_eq!(file.filename.as_deref(), Some("audio.ogg"));
    assert_eq!(file.content_type.as_deref(), Some("audio/ogg"));
    assert_eq!(file.data, audio);
}

#[tokio::test]
async fn a_success_returns_the_text_and_any_reported_duration() {
    let (url, fake) = start();
    fake.script([
        Reply::ok(r#"{"text":"with usage","usage":{"type":"duration","seconds":3.2}}"#),
        Reply::ok(r#"{"text":"without usage","extra":true}"#),
    ]);
    let client = transcriber(&url, &[]);
    let first = client
        .transcribe(input(AudioContainer::Mp3, mp3_of(4_000)), far())
        .await
        .unwrap();
    assert_eq!(
        (first.text.as_str(), first.usage_seconds),
        ("with usage", Some(4))
    );
    let second = client
        .transcribe(input(AudioContainer::Mp3, mp3_of(4_000)), far())
        .await
        .unwrap();
    assert_eq!(
        (second.text.as_str(), second.usage_seconds),
        ("without usage", None)
    );
}

#[tokio::test]
async fn amr_is_unsupported_without_a_request() {
    let (url, fake) = start();
    let source = FakeSource::new();
    let service = service(&source, &url, &[WITH_OGG]);
    let mut amr = b"#!AMR\n".to_vec();
    amr.resize(8_000, 0);
    let target = audio_note(&source, "stt-amr", "amr", amr, |_| {});
    assert_eq!(
        failed(service.derive(&target, 0, Artifact::Transcript, LONG).await),
        DeriveError::UnsupportedFormat
    );
    assert_eq!(fake.requests(), 0);
}

#[tokio::test]
async fn ogg_is_refused_until_it_is_configured() {
    let (url, fake) = start();
    let audio = ogg(8_000);
    assert!(matches!(sniff(&audio).mime, "audio/ogg" | "audio/opus"));
    let default = transcriber(&url, &[]);
    assert!(!default.accepts(AudioContainer::Ogg));
    assert_eq!(
        default
            .transcribe(input(AudioContainer::Ogg, audio.clone()), far())
            .await
            .unwrap_err(),
        TranscribeError::Unsupported
    );
    assert_eq!(
        DeriveError::from(TranscribeError::Unsupported).as_str(),
        "unsupported_format"
    );
    assert_eq!(fake.requests(), 0);
    let with_ogg = transcriber(&url, &[WITH_OGG]);
    assert!(with_ogg.accepts(AudioContainer::Ogg));
    with_ogg
        .transcribe(input(AudioContainer::Ogg, audio), far())
        .await
        .unwrap();
    assert_eq!(fake.requests(), 1);
}

#[tokio::test]
async fn mp3_aliases_and_the_default_formats_admit_their_containers() {
    let (url, _fake) = start();
    let default = transcriber(&url, &[]);
    for container in [
        AudioContainer::Mp3,
        AudioContainer::Mp4,
        AudioContainer::M4a,
        AudioContainer::Wav,
        AudioContainer::Webm,
    ] {
        assert!(default.accepts(container), "{container:?}");
    }
    assert!(!default.accepts(AudioContainer::Flac));
    for alias in ["mpeg", "mpga"] {
        let only = transcriber(&url, &[("CMAEG_STT_FORMATS", alias)]);
        assert!(only.accepts(AudioContainer::Mp3), "{alias}");
        assert!(!only.accepts(AudioContainer::Wav), "{alias}");
    }
}

/// Transcribe a small MP3 after scripting `replies`, returning the result and
/// how many requests the endpoint received.
async fn scripted(
    replies: Vec<Reply>,
    deadline: Instant,
    extra: &[(&'static str, &str)],
) -> (
    Result<cmaeg::media::transcribe::Transcript, TranscribeError>,
    usize,
) {
    let (url, fake) = start();
    fake.script(replies);
    let result = transcriber(&url, extra)
        .transcribe(input(AudioContainer::Mp3, mp3_of(4_000)), deadline)
        .await;
    (result, fake.requests())
}

#[tokio::test]
async fn a_rate_limit_then_success_succeeds_on_the_retry() {
    let (result, requests) = scripted(vec![Reply::status(429, "{}")], far(), &[]).await;
    assert_eq!(result.unwrap().text, SPOKEN);
    assert_eq!(requests, 2);
}

#[tokio::test]
async fn two_rate_limits_are_rate_limited() {
    let replies = vec![Reply::status(429, "{}"), Reply::status(429, "{}")];
    let (result, requests) = scripted(replies, far(), &[]).await;
    assert_eq!(result.unwrap_err(), TranscribeError::RateLimited);
    assert_eq!(
        DeriveError::from(TranscribeError::RateLimited).as_str(),
        "rate_limited"
    );
    assert_eq!(requests, 2);
}

#[tokio::test]
async fn two_server_errors_fail_the_transcriber() {
    let replies = vec![Reply::status(500, "{}"), Reply::status(503, "{}")];
    let (result, requests) = scripted(replies, far(), &[]).await;
    assert_eq!(result.unwrap_err(), TranscribeError::Failed);
    assert_eq!(
        DeriveError::from(TranscribeError::Failed).as_str(),
        "transcriber_failed"
    );
    assert_eq!(requests, 2);
}

#[tokio::test]
async fn no_retry_is_attempted_without_two_seconds_left() {
    let deadline = Instant::now() + Duration::from_millis(1_500);
    let (result, requests) = scripted(vec![Reply::status(429, "{}")], deadline, &[]).await;
    assert_eq!(result.unwrap_err(), TranscribeError::RateLimited);
    assert_eq!(requests, 1);
}

#[tokio::test]
async fn a_client_error_fails_without_a_retry() {
    let (result, requests) = scripted(vec![Reply::status(400, "{}")], far(), &[]).await;
    assert_eq!(result.unwrap_err(), TranscribeError::Failed);
    assert_eq!(requests, 1);
}

#[tokio::test]
async fn a_slow_response_times_out_at_the_attempt_timeout() {
    let slow = Reply {
        delay: Duration::from_secs(5),
        ..Reply::ok(r#"{"text":"late"}"#)
    };
    let started = Instant::now();
    let (result, requests) = scripted(vec![slow], far(), &[("CMAEG_STT_TIMEOUT_MS", "1000")]).await;
    assert_eq!(result.unwrap_err(), TranscribeError::TimedOut);
    assert_eq!(requests, 1);
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn an_oversized_or_textless_response_fails() {
    let oversized = format!(r#"{{"text":"{}"}}"#, "a".repeat(300 * 1024));
    for body in [
        oversized.as_str(),
        r#"{"transcript":"no text field"}"#,
        "not json",
    ] {
        let (result, requests) = scripted(vec![Reply::ok(body)], far(), &[]).await;
        assert_eq!(result.unwrap_err(), TranscribeError::Failed);
        assert_eq!(requests, 1);
    }
}

#[tokio::test]
async fn a_fifth_concurrent_call_waits_for_a_permit() {
    assert_eq!(MAX_CONCURRENT_CALLS, 4);
    let (url, fake) = start();
    fake.close();
    let client = transcriber(&url, &[]);
    let calls: Vec<_> = (0..5)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move {
                client
                    .transcribe(input(AudioContainer::Mp3, mp3_of(4_000)), far())
                    .await
            })
        })
        .collect();
    until_received(&fake, 4).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fake.requests(), 4, "the fifth call did not wait");
    fake.open();
    for call in calls {
        assert_eq!(call.await.unwrap().unwrap().text, SPOKEN);
    }
    assert_eq!(fake.requests(), 5);
    assert_eq!(fake.peak(), 4);
}

#[tokio::test]
async fn a_voice_note_is_fetched_metered_and_transcribed_end_to_end() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let (url, fake) = start();
    fake.close();
    let source = FakeSource::new();
    let service = Arc::new(service(&source, &url, &[WITH_OGG]));
    let target = audio_note(&source, "stt-e2e", "voice.oga", ogg(2_400_000), |_| {});
    let job = {
        let (service, target) = (service.clone(), target.clone());
        tokio::spawn(async move { service.derive(&target, 0, Artifact::Transcript, LONG).await })
    };
    until_received(&fake, 1).await;
    // While the call is open, only the fetch reservation is held: the upload
    // streams the fetched bytes rather than reserving a copy.
    let inflight = "cmaeg_media_inflight_bytes";
    let reserved = 2_400_000_u64.div_ceil(1024) * 1024;
    assert_eq!(sample(&handle.render(), inflight), Some(reserved as f64));
    fake.open();
    let derived = ready(job.await.unwrap());
    let Derived::Transcript { document, .. } = &*derived else {
        panic!("{derived:?}");
    };
    assert_eq!(document.pages, [SPOKEN]);
    assert_eq!(sample(&handle.render(), inflight), Some(0.0));
    let text = handle.render();
    assert_eq!(sample(&text, "cmaeg_stt_audio_seconds_total"), Some(600.0));
    let [request] = &fake.received()[..] else {
        panic!("expected one request");
    };
    assert_eq!(request.content_length, Some(request.body.len() as u64));
    let file = request
        .parts()
        .into_iter()
        .find(|part| part.name == "file")
        .unwrap();
    assert_eq!(file.data.len(), 2_400_000);
    assert_eq!(file.filename.as_deref(), Some("audio.ogg"));
}

#[tokio::test]
async fn a_spent_channel_is_rate_limited_while_another_channel_succeeds() {
    let (url, fake) = start();
    let source = FakeSource::new();
    let service = service(
        &source,
        &url,
        &[WITH_OGG, ("CMAEG_STT_CHANNEL_SECONDS_PER_HOUR", "600")],
    );
    let first = audio_note(&source, "stt-spent", "first.oga", ogg(2_400_000), |_| {});
    ready(service.derive(&first, 0, Artifact::Transcript, LONG).await);
    let second = audio_note(&source, "stt-spent", "second.oga", ogg(2_400_000), |_| {});
    assert_eq!(
        failed(service.derive(&second, 0, Artifact::Transcript, LONG).await),
        DeriveError::RateLimited
    );
    let other = audio_note(&source, "stt-fresh", "other.oga", ogg(2_400_000), |_| {});
    ready(service.derive(&other, 0, Artifact::Transcript, LONG).await);
    assert_eq!(fake.requests(), 2);
}

#[tokio::test]
async fn neither_the_key_the_url_nor_the_transcript_reaches_a_log() {
    let (url, fake) = start();
    fake.script([Reply::status(400, r#"{"error":"bad"}"#)]);
    let source = FakeSource::new();
    let service = service(&source, &url, &[WITH_OGG]);
    let target = audio_note(&source, "stt-leak", "leak", ogg(8_000), |_| {});
    let (guard, logs) = capture_logs(TRACE_ENV);
    assert_eq!(
        failed(service.derive(&target, 0, Artifact::Transcript, LONG).await),
        DeriveError::TranscriberFailed
    );
    let second = audio_note(&source, "stt-leak", "leak-2", ogg(8_000), |_| {});
    ready(service.derive(&second, 0, Artifact::Transcript, LONG).await);
    // A refused connection fails in transport.
    let unreachable = transcriber("http://127.0.0.1:9/v1/audio/transcriptions", &[]);
    assert_eq!(
        unreachable
            .transcribe(input(AudioContainer::Mp3, mp3_of(4_000)), far())
            .await
            .unwrap_err(),
        TranscribeError::Failed
    );
    drop(guard);
    let text = logs.text();
    assert!(text.contains("cmaeg.media.stt"), "{text}");
    assert!(text.contains("metered_seconds=2"), "{text}");
    assert!(
        text.contains(&channel_hash(target.channel.id.as_str())),
        "{text}"
    );
    assert!(text.contains("speech-to-text call finished"), "{text}");
    let origin = url.trim_end_matches(super::stt_fake::PATH);
    assert_absent(
        &text,
        &[KEY, &url, origin, "/v1/audio/transcriptions", SPOKEN],
    );
    let config = super::stt_fake::config_for(&url, &[]).unwrap();
    let shown = format!(
        "{:?} {config:?} {:?}",
        transcriber(&url, &[]),
        config.endpoint
    );
    assert_absent(&shown, &[KEY, &url, origin, "/v1/audio/transcriptions"]);
    assert!(shown.contains("url_configured: true"), "{shown}");
}
