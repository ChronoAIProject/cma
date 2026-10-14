//! readEventAttachment and event context with the media transformer
//! configured: the production keyframe and transcode adapters over a
//! scripted transformer, wired as `compose` wires a configured one.
//! Keyframes and their positions, the platform-thumbnail fallback, the
//! reported visual capability, and the notice of a transcript cut at the
//! speech-to-text limit.
use super::{
    media::image_fixtures as images,
    media_support::{mp3, native, relayed, MediaSetup},
    support::App,
};
use bytes::Bytes;
use cmaeg::media::transform::{
    outcome::TransformOutcome, AudioRequest, KeyframeRequest, Keyframes, MediaTransformer, Pcm,
    TransformError,
};
use cmaeg_core::activity::{AttachmentKind, AttachmentSource};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::Arc;

/// At the default speech-to-text limits a transcode is cut at ten minutes.
const CUT_NOTICE: &str = "transcript covers only the first 10:00 of the audio";
/// The head of an MP4 the sniffer recognizes as video.
const MP4: &[u8] = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom";

type KeyframeScript = fn() -> Result<Keyframes, TransformError>;

/// Answers keyframe sets from a script and cuts every transcode at its
/// cap, as FFmpeg does for audio longer than the speech-to-text limit.
struct Scripted {
    keyframes: KeyframeScript,
}

impl MediaTransformer for Scripted {
    fn keyframes(
        &self,
        request: KeyframeRequest,
    ) -> BoxFuture<'_, Result<Keyframes, TransformError>> {
        assert_eq!(request.frames, 6, "CMAEG_MEDIA_VIDEO_FRAMES at its default");
        let set = (self.keyframes)();
        Box::pin(async move { set })
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

fn transformer(keyframes: KeyframeScript) -> Arc<dyn MediaTransformer> {
    Arc::new(Scripted { keyframes })
}

/// Three 1280×720 frames, in the order a keyframe set returns them.
fn three_frames() -> Result<Keyframes, TransformError> {
    let frame = |noise| Bytes::from(images::jpeg(&images::rgb(1280, 720, noise), 85));
    Ok(Keyframes {
        frames: vec![(frame(1), 500), (frame(2), 2_500), (frame(3), 4_500)],
        duration_ms: Some(5_000),
        partial: false,
    })
}

/// The memory watchdog stopped the set.
fn stopped_by_the_watchdog() -> Result<Keyframes, TransformError> {
    Err(TransformError::job(TransformOutcome::ResourceLimit))
}

fn no_frames() -> Result<Keyframes, TransformError> {
    Err(TransformError::NoFrames)
}

fn relay(index: u8) -> AttachmentSource {
    AttachmentSource::NyxidRelay { index }
}

/// The REST view of attachment `index` with `query`.
async fn view(app: &App, index: &str, query: &str) -> Value {
    let response = app.attachment(index, query).await;
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

/// The MCP view of attachment `index` and the image items that follow it.
async fn mcp_read(app: &App, index: u8, view: &str) -> (Value, Vec<Value>) {
    let response = app
        .rpc(json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"readEventAttachment","arguments":{"event_ref":app.event_ref.as_str(),"index":index,"view":view}}}))
        .await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let content = body["result"]["content"].as_array().unwrap();
    let view = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    (view, content[1..].to_vec())
}

/// `inbound_media.visuals` of the app's event context.
async fn visuals(app: &App) -> Value {
    let response = app
        .fixture
        .client
        .get(app.event_url("context"))
        .bearer_auth(&app.key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json::<Value>().await.unwrap()["capabilities"]["inbound_media"]["visuals"].clone()
}

#[tokio::test]
async fn frames_are_keyframes_with_their_positions() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Video)])
        .transformer(transformer(three_frames));
    setup.fakes.source.serve(&relay(0), Ok(MP4.to_vec()));
    let app = App::with_media(setup).await;
    // A relayed video has no platform thumbnail: only the transformer
    // gives it frames.
    assert_eq!(visuals(&app).await, "keyframes");
    let frames = view(&app, "0", "?view=frames").await;
    assert_eq!(frames["status"], "ready");
    assert!(frames.get("reason").is_none() && frames.get("notice").is_none());
    let images = frames["images"].as_array().unwrap();
    let positions: Vec<_> = images
        .iter()
        .map(|image| (image["origin"].clone(), image["at_ms"].clone()))
        .collect();
    assert_eq!(
        positions,
        [500, 2_500, 4_500].map(|at_ms| (json!("keyframe"), json!(at_ms)))
    );
    // Every keyframe is normalized to a preview of at most 768 px.
    assert!(
        images
            .iter()
            .all(|image| (image["width"].as_u64(), image["height"].as_u64())
                == (Some(768), Some(432)))
    );
    let (mcp, items) = mcp_read(&app, 0, "frames").await;
    assert_eq!(mcp["images"], frames["images"]);
    assert_eq!(items.len(), 3);
    assert!(items
        .iter()
        .all(|item| item["type"] == "image" && item["_meta"]["codex/imageDetail"] == "auto"));
}

#[tokio::test]
async fn failed_keyframes_fall_back_to_the_platform_thumbnail() {
    let mut clip = native("clip", AttachmentKind::Video);
    let thumbnail = AttachmentSource::Telegram {
        file_id: "file-thumb".into(),
        file_unique_id: "thumb".into(),
    };
    clip.thumbnail = Some(thumbnail.clone());
    let original = clip.source.clone().unwrap();
    let setup = MediaSetup::new(vec![clip])
        .native()
        .transformer(transformer(stopped_by_the_watchdog));
    setup.fakes.source.serve(&original, Ok(MP4.to_vec()));
    setup
        .fakes
        .source
        .serve(&thumbnail, Ok(images::jpeg(&images::rgb(320, 180, 8), 80)));
    let app = App::with_media(setup).await;
    assert_eq!(visuals(&app).await, "keyframes");
    let frames = view(&app, "0", "?view=frames").await;
    assert_eq!(frames["status"], "ready");
    let images = frames["images"].as_array().unwrap();
    assert_eq!(images.len(), 1);
    // The fallback image has no position, so it is the platform's thumbnail.
    assert_eq!(images[0]["origin"], "platform_thumbnail");
    assert!(images[0].get("at_ms").is_none());
    assert_eq!(
        (images[0]["width"].as_u64(), images[0]["height"].as_u64()),
        (Some(320), Some(180))
    );
}

#[tokio::test]
async fn failed_keyframes_without_a_thumbnail_report_the_transform_reason() {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Video)])
        .transformer(transformer(stopped_by_the_watchdog));
    setup.fakes.source.serve(&relay(0), Ok(MP4.to_vec()));
    let app = App::with_media(setup).await;
    let frames = view(&app, "0", "?view=frames").await;
    assert_eq!(
        (frames["status"].as_str(), frames["reason"].as_str()),
        (Some("unavailable"), Some("transform_failed"))
    );
    assert!(frames.get("images").is_none() && frames.get("notice").is_none());
}

#[tokio::test]
async fn a_transcript_cut_at_the_limit_carries_the_notice() {
    let setup = MediaSetup::new(vec![
        relayed(0, AttachmentKind::Audio),
        relayed(1, AttachmentKind::Video),
    ])
    .transformer(transformer(no_frames));
    setup.fakes.source.serve(&relay(0), Ok(mp3()));
    setup.fakes.source.serve(&relay(1), Ok(MP4.to_vec()));
    setup.fakes.transcriber.say("the first ten minutes");
    let app = App::with_media(setup).await;
    let audio = view(&app, "0", "").await;
    assert_eq!(audio["status"], "ready");
    assert_eq!(audio["text"]["source"], "transcript");
    assert_eq!(audio["text"]["content"], "the first ten minutes");
    // The transcript's text is whole; it is the audio that was cut.
    assert_eq!(audio["text"]["truncated"], false);
    assert_eq!(audio["notice"], CUT_NOTICE);
    let (mcp, items) = mcp_read(&app, 0, "text").await;
    assert_eq!(mcp["notice"], CUT_NOTICE);
    assert!(items.is_empty());
    // A video's auto view reports its missing frames first and the cut last.
    let video = view(&app, "1", "").await;
    assert_eq!(video["status"], "ready");
    assert_eq!(video["text"]["content"], "the first ten minutes");
    assert_eq!(
        video["notice"],
        format!("frames unavailable: timed_out; {CUT_NOTICE}")
    );
}
