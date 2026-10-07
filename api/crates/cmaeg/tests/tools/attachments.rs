//! readEventAttachment over REST: views, authority, pending work, budgets
//! and the reasons an attachment is unavailable.
use super::{
    media::{
        image_fixtures as images,
        support::{assert_absent, capture_logs, TRACE_ENV},
    },
    media_support::{mp3, relayed, FrameSpec, MediaSetup},
    support::{problem, App},
};
use cmaeg::{media::derive::DeriveError, tools::AttachmentUnavailableReason};
use cmaeg_core::{
    activity::{AttachmentKind, AttachmentSource},
    seal::EventRef,
};
use serde_json::Value;
use std::time::Duration;
use tokio::time::Instant;

const MIB: u64 = 1024 * 1024;

fn relay(index: u8) -> AttachmentSource {
    AttachmentSource::NyxidRelay { index }
}

fn photo() -> Vec<u8> {
    images::jpeg(&images::rgb(1200, 800, 16), 85)
}

/// Two 16 KiB pages and a short third of numbered lines.
fn document() -> Vec<u8> {
    (0..4_000)
        .map(|line| format!("line {line:05}\n"))
        .collect::<String>()
        .into_bytes()
}

/// A 200 view, checked to be plain JSON.
async fn view(response: reqwest::Response) -> Value {
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(response.headers()["cmaeg-protocol-version"], "1");
    response.json().await.unwrap()
}

#[tokio::test]
async fn rest_reads_image_metadata_without_bytes() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Image)]);
    setup.fakes.source.serve(&relay(0), Ok(photo()));
    let app = App::with_media(setup).await;
    let response = app.attachment("0", "").await;
    assert!(response.headers()["server-timing"]
        .to_str()
        .unwrap()
        .starts_with("deliver;dur="));
    let body = view(response).await;
    assert_eq!(body["status"], "ready");
    assert_eq!(body["kind"], "image");
    let image = &body["images"][0];
    assert_eq!(image["origin"], "normalized");
    assert_eq!(image["mime_type"], "image/jpeg");
    assert_eq!(
        (image["width"].as_u64(), image["height"].as_u64()),
        (Some(1200), Some(800))
    );
    assert!(image["bytes"].as_u64().unwrap() <= MIB);
    assert!(image.get("at_ms").is_none());
    // Metadata only: no base64 JPEG (it would start "/9j/") and no data.
    let text = body.to_string();
    assert!(!text.contains("/9j/") && !text.contains("\"data\""));
    assert!(text.len() < 1024);
}

#[tokio::test]
async fn rest_reads_text_pages() {
    let mut file = relayed(0, AttachmentKind::File);
    file.filename = Some("notes.txt".into());
    let setup = MediaSetup::new(vec![file]);
    setup.fakes.source.serve(&relay(0), Ok(document()));
    let app = App::with_media(setup).await;
    let first = view(app.attachment("0", "?view=text").await).await;
    assert_eq!(first["status"], "ready");
    assert_eq!(first["detected_type"], "text/plain");
    assert_eq!(first["filename"], "notes.txt");
    assert_eq!(first["text"]["source"], "document");
    assert_eq!(
        (
            first["text"]["page"].as_u64(),
            first["text"]["pages"].as_u64()
        ),
        (Some(1), Some(3))
    );
    assert!(first["text"]["content"]
        .as_str()
        .unwrap()
        .starts_with("line 00000\n"));
    assert_eq!(first["text"]["truncated"], false);
    let second = view(app.attachment("0", "?page=2").await).await;
    assert_eq!(second["text"]["page"], 2);
    assert!(second["text"]["content"]
        .as_str()
        .unwrap()
        .starts_with("01489\nline 01490\n"));
    // Past the last page: not found, still saying how many pages exist.
    let past = view(app.attachment("0", "?page=4").await).await;
    assert_eq!(
        (past["status"].as_str(), past["reason"].as_str()),
        (Some("unavailable"), Some("not_found"))
    );
    assert_eq!(
        (
            past["text"]["page"].as_u64(),
            past["text"]["pages"].as_u64()
        ),
        (Some(4), Some(3))
    );
    // Beyond the 64-page cap of any document the request itself is invalid.
    for query in ["?page=99", "?page=0", "?view=hologram", "?unknown=1"] {
        let refused = problem(app.attachment("0", query).await, 400, "invalid_request").await;
        assert_eq!(refused["detail"], "invalid attachment request");
    }
}

#[tokio::test]
async fn rest_reads_transcript() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Audio)]);
    setup.fakes.source.serve(&relay(0), Ok(mp3()));
    setup.fakes.transcriber.say("hello from the voice note");
    let app = App::with_media(setup).await;
    let body = view(app.attachment("0", "").await).await;
    assert_eq!(body["status"], "ready");
    assert_eq!(body["text"]["source"], "transcript");
    assert_eq!(body["text"]["content"], "hello from the voice note");
    assert_eq!(
        (
            body["text"]["page"].as_u64(),
            body["text"]["pages"].as_u64()
        ),
        (Some(1), Some(1))
    );
    // Untruncated audio carries no notice.
    assert!(body.get("notice").is_none());
}

#[tokio::test]
async fn rest_pending_then_ready() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Audio)]);
    let fakes = setup.fakes.clone();
    fakes.source.serve(&relay(0), Ok(mp3()));
    fakes.transcriber.delay(Duration::from_secs(10));
    let app = App::with_media(setup).await;
    let request = app
        .fixture
        .client
        .get(format!("{}/0", app.event_url("attachments")))
        .bearer_auth(&app.key);
    let first = tokio::spawn(async move { request.send().await.unwrap() });
    // Pause only once the network part of the call is done and the job
    // runs, so paused time drives just the 8 s wait and the job.
    while fakes.transcriber.calls() == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::pause();
    let first = first.await.unwrap();
    let retry_after = first.headers()["retry-after"].to_str().unwrap().to_owned();
    let pending = view(first).await;
    assert_eq!(pending["status"], "pending");
    let after = pending["retry_after_ms"].as_u64().unwrap();
    assert_eq!(retry_after, after.div_ceil(1000).to_string());
    assert!(pending.get("text").is_none());
    // The job continues into the cache.
    tokio::time::sleep(Duration::from_secs(3)).await;
    tokio::time::resume();
    let ready = view(app.attachment("0", "").await).await;
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["text"]["content"], "fake transcript");
    assert_eq!(fakes.transcriber.calls(), 1);
    assert_eq!(fakes.source.calls(), 1);
}

#[tokio::test]
async fn rest_refuses_unverified_context() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Image)]).without_digest();
    setup.fakes.source.serve(&relay(0), Ok(photo()));
    let fakes = setup.fakes.clone();
    let app = App::with_media(setup).await;
    let refused = problem(app.attachment("0", "").await, 503, "media_unavailable").await;
    assert_eq!(refused["detail"], "event_context_unverified");
    assert_eq!(fakes.source.calls(), 0);
}

#[tokio::test]
async fn rest_changed_activity_fails_as_context_reads_do() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Image)]);
    setup.fakes.source.serve(&relay(0), Ok(photo()));
    let fakes = setup.fakes.clone();
    let app = App::with_media(setup).await;
    // A digest the provider's persisted activity does not hash to is what an
    // activity changed after sealing looks like to the gateway.
    let mut payload = app
        .event_ref
        .open(&app.state.keys, super::relay_support::now())
        .unwrap()
        .payload;
    payload.activity_sha256 = Some("0".repeat(64));
    let changed = EventRef::seal(&payload, &app.state.keys).unwrap();
    let response = app.attachment_under(changed.as_str(), "0", "").await;
    let refused = problem(response, 503, "provider_unavailable").await;
    assert_eq!(refused["detail"], "event_context_unavailable");
    assert_eq!(fakes.source.calls(), 0);
}

#[tokio::test]
async fn rest_index_out_of_range() {
    let app = App::with_media(MediaSetup::new(vec![relayed(0, AttachmentKind::Image)])).await;
    let missing = problem(app.attachment("1", "").await, 404, "attachment_not_found").await;
    assert_eq!(missing["detail"], "attachment_not_found");
    for index in ["16", "256", "-1", "x"] {
        problem(app.attachment(index, "").await, 400, "invalid_request").await;
    }
}

#[tokio::test]
async fn rest_expired_event_ref() {
    let app = App::with_media(MediaSetup::new(vec![relayed(0, AttachmentKind::Image)])).await;
    let mut payload = app
        .event_ref
        .open(&app.state.keys, super::relay_support::now())
        .unwrap()
        .payload;
    payload.expires_at = super::relay_support::now() - 1;
    let expired = EventRef::seal(&payload, &app.state.keys).unwrap();
    let response = app.attachment_under(expired.as_str(), "0", "").await;
    problem(response, 410, "event_ref_expired").await;
    let tampered = format!("{}x", app.event_ref.as_str());
    let response = app.attachment_under(&tampered, "0", "").await;
    problem(response, 400, "event_ref_invalid").await;
}

#[tokio::test]
async fn rest_wrong_or_missing_key() {
    let app = App::with_media(MediaSetup::new(vec![relayed(0, AttachmentKind::Image)])).await;
    let url = format!("{}/0", app.event_url("attachments"));
    let missing = app.fixture.client.get(&url).send().await.unwrap();
    problem(missing, 401, "not_authenticated").await;
    let wrong = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    let response = app
        .fixture
        .client
        .get(&url)
        .bearer_auth(wrong)
        .send()
        .await
        .unwrap();
    problem(response, 403, "forbidden").await;
}

#[tokio::test]
async fn rest_media_disabled() {
    let app = App::new().await;
    let refused = problem(app.attachment("0", "").await, 503, "media_unavailable").await;
    assert_eq!(refused["detail"], "media_disabled");
}

#[tokio::test]
async fn rest_rate_limited_is_an_unavailable_reason() {
    let setup = MediaSetup::new(vec![
        relayed(0, AttachmentKind::Audio),
        relayed(1, AttachmentKind::Audio),
    ])
    .limits(|limits| limits.fetch_per_minute = 1);
    setup.fakes.source.serve(&relay(0), Ok(mp3()));
    setup.fakes.source.serve(&relay(1), Ok(mp3()));
    let app = App::with_media(setup).await;
    assert_eq!(view(app.attachment("0", "").await).await["status"], "ready");
    let limited = view(app.attachment("1", "").await).await;
    assert_eq!(limited["status"], "unavailable");
    assert_eq!(limited["reason"], "rate_limited");
    // A cached artifact needs no fetch, so the quota does not apply.
    assert_eq!(view(app.attachment("0", "").await).await["status"], "ready");
}

#[tokio::test]
async fn rest_response_budget_exhausted() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Audio)])
        .limits(|limits| limits.response_bytes = 4 * MIB);
    setup.fakes.source.serve(&relay(0), Ok(mp3()));
    let app = App::with_media(setup).await;
    let media = app.media.clone().unwrap();
    let held = media
        .responses()
        .acquire(4 * MIB, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    let response = app.attachment("0", "").await;
    assert_eq!(response.headers()["retry-after"], "1");
    let refused = problem(response, 429, "rate_limited").await;
    assert_eq!(refused["detail"], "response_budget");
    drop(held);
    assert_eq!(view(app.attachment("0", "").await).await["status"], "ready");
}

#[tokio::test]
async fn rest_permit_released_when_client_stops_reading() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Audio)]);
    setup.fakes.source.serve(&relay(0), Ok(mp3()));
    let full = setup.limits.response_bytes;
    let app = App::with_media(setup).await;
    let response = app.attachment("0", "").await;
    assert_eq!(response.status(), 200);
    // The body is never polled; the forwarding deadline still frees it.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(16)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        app.media.as_ref().unwrap().responses().available_bytes(),
        full
    );
    drop(response);
}

#[test]
fn every_derive_code_maps_to_a_reason() {
    use AttachmentUnavailableReason as Reason;
    let table = [
        (DeriveError::NotRetrievable, Reason::NotRetrievable),
        (DeriveError::NotFound, Reason::NotFound),
        (DeriveError::TooLarge, Reason::TooLarge),
        (DeriveError::TooLargePixels, Reason::TooLargePixels),
        (DeriveError::UnsupportedFormat, Reason::UnsupportedFormat),
        (DeriveError::DecodeFailed, Reason::DecodeFailed),
        (DeriveError::TimedOut, Reason::TimedOut),
        (DeriveError::SourceUnavailable, Reason::SourceUnavailable),
        (
            DeriveError::TranscriberDisabled,
            Reason::TranscriberDisabled,
        ),
        (DeriveError::TranscriberFailed, Reason::TranscriberFailed),
        (DeriveError::TooLong, Reason::TooLong),
        (DeriveError::RateLimited, Reason::RateLimited),
        (DeriveError::TransformFailed, Reason::TransformFailed),
    ];
    for (error, reason) in table {
        assert_eq!(Reason::from(error), reason);
        // The agent sees the same code operators see on the derive metric.
        assert_eq!(reason.as_str(), error.as_str());
        assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
    }
}

#[tokio::test]
async fn visual_origin_follows_each_image() {
    let setup = MediaSetup::new(vec![
        relayed(0, AttachmentKind::Video),
        relayed(1, AttachmentKind::Image),
    ]);
    setup.fakes.visuals.serve(vec![
        FrameSpec {
            len: 4_096,
            at_ms: Some(1_500),
        },
        FrameSpec {
            len: 2_048,
            at_ms: None,
        },
    ]);
    setup.fakes.source.serve(&relay(1), Ok(photo()));
    let app = App::with_media(setup).await;
    let frames = view(app.attachment("0", "?view=frames").await).await;
    assert_eq!(frames["status"], "ready");
    let origins: Vec<_> = frames["images"]
        .as_array()
        .unwrap()
        .iter()
        .map(|image| image["origin"].clone())
        .collect();
    assert_eq!(origins, ["platform_thumbnail", "keyframe"]);
    assert!(frames["images"][0].get("at_ms").is_none());
    assert_eq!(frames["images"][1]["at_ms"], 1_500);
    assert_eq!(frames["images"][1]["bytes"], 4_096);
    let image = view(app.attachment("1", "?view=image").await).await;
    assert_eq!(image["images"][0]["origin"], "normalized");
}

#[tokio::test]
async fn video_auto_reports_the_missing_part_in_the_notice() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Video)]);
    setup.fakes.source.serve(
        &relay(0),
        Ok(b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom".to_vec()),
    );
    setup.fakes.transcriber.say("spoken words");
    let app = App::with_media(setup).await;
    let auto = view(app.attachment("0", "").await).await;
    assert_eq!(auto["status"], "ready");
    assert_eq!(auto["text"]["content"], "spoken words");
    assert_eq!(auto["notice"], "frames unavailable: not_retrievable");
    let frames = view(app.attachment("0", "?view=frames").await).await;
    assert_eq!(
        (frames["status"].as_str(), frames["reason"].as_str()),
        (Some("unavailable"), Some("not_retrievable"))
    );
    assert_eq!(
        frames["notice"],
        "frames are not available for this attachment on this gateway; the transcript is"
    );
    let image = view(app.attachment("0", "?view=image").await).await;
    assert_eq!(image["reason"], "unsupported_format");
    assert_eq!(image["notice"], "use view \"frames\"");
}

#[tokio::test]
async fn download_view_is_link_unavailable() {
    let app = App::with_media(MediaSetup::new(vec![relayed(0, AttachmentKind::File)])).await;
    let body = view(app.attachment("0", "?view=download").await).await;
    assert_eq!(
        (body["status"].as_str(), body["reason"].as_str()),
        (Some("unavailable"), Some("link_unavailable"))
    );
    // No derivation runs for a link, not even the file's type sniff.
    assert!(body.get("detected_type").is_none());
}

#[tokio::test]
async fn binary_files_return_metadata_only() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::File)]);
    setup.fakes.source.serve(
        &relay(0),
        Ok(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n1 0 obj".to_vec()),
    );
    let app = App::with_media(setup).await;
    let body = view(app.attachment("0", "").await).await;
    assert_eq!(body["status"], "ready");
    assert_eq!(body["detected_type"], "application/pdf");
    assert_eq!(
        body["notice"],
        "binary file: only metadata is available to this agent"
    );
    assert!(body.get("text").is_none() && body.get("images").is_none());
    for view_kind in ["image", "text", "frames"] {
        let refused = view(app.attachment("0", &format!("?view={view_kind}")).await).await;
        assert_eq!(refused["reason"], "unsupported_format");
    }
}

#[tokio::test]
async fn an_image_under_frames_is_unsupported_with_a_hint() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Image)]);
    setup.fakes.source.serve(&relay(0), Ok(photo()));
    let app = App::with_media(setup).await;
    for query in ["?view=frames", "?view=text"] {
        let body = view(app.attachment("0", query).await).await;
        assert_eq!(
            (body["status"].as_str(), body["reason"].as_str()),
            (Some("unavailable"), Some("unsupported_format"))
        );
        assert_eq!(body["notice"], "use view \"image\"");
    }
}

#[tokio::test]
async fn reads_never_log_references_names_or_content() {
    let mut file = relayed(1, AttachmentKind::File);
    file.filename = Some("quarterly-secret-plan.txt".into());
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Audio), file]);
    setup.fakes.source.serve(&relay(0), Ok(mp3()));
    setup
        .fakes
        .source
        .serve(&relay(1), Ok(b"confidential document words".to_vec()));
    setup.fakes.transcriber.say("transcribed private words");
    let (guard, logs) = capture_logs(TRACE_ENV);
    let app = App::with_media(setup).await;
    view(app.attachment("0", "").await).await;
    view(app.attachment("1", "").await).await;
    drop(guard);
    let text = logs.text();
    assert!(text.contains("cmaeg.media.read"));
    assert_absent(
        &text,
        &[
            app.event_ref.as_str(),
            &app.key,
            "quarterly-secret-plan",
            "transcribed private words",
            "confidential document words",
        ],
    );
}
