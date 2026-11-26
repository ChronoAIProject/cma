//! Lark media through the NyxID relay, sent as NyxID serializes it: the raw
//! content is a JSON string and the envelope lists the message's one resource.
//! With `CMAEG_MEDIA_ENABLED=true` no attachment fact may turn a callback into
//! a 400; with it off every media type is refused exactly as before. The
//! helpers here also build the rich-text posts of `lark_post.rs`.
use super::{
    nyxid_shapes::{attachment, counter},
    support::*,
};
use cmaeg::turns::{
    render_input::render,
    render_media::{Audience, RenderMedia},
};
use cmaeg_core::activity::{ActivityKind, Attachment, AttachmentKind, MediaVariant};
use serde_json::{json, Value};

pub const IMAGE_KEY: &str = "img_v3_02ab_fixture";
const FILE_KEY: &str = "file_v3_00ab_fixture";
const RESOURCES: &str = "https://open.larksuite.com/open-apis/im/v1/messages/om_message/resources";

/// NyxID's envelope entry for the resource `key` at original position `index`.
pub fn resource(index: usize, field: &str, key: &str, facts: Value) -> Value {
    let kind = if field == "image_key" {
        "image"
    } else {
        "file"
    };
    let mut facts = facts;
    facts[field] = key.into();
    facts["platform_message_id"] = "om_message".into();
    attachment(
        index,
        kind,
        &format!("{RESOURCES}/{key}?type={kind}"),
        facts,
    )
}

/// A Lark message of `message_type` whose raw content is `content`, with NyxID's
/// `content.type`, a null `content.text` and the envelope `items`.
pub fn lark(message_type: &str, content: Value, nyxid_type: &str, items: Vec<Value>) -> Envelope {
    let mut envelope = Envelope::on("lark");
    envelope.lark_message()["message_type"] = message_type.into();
    envelope.lark_message()["content"] = content.to_string().into();
    envelope.payload["content"] = json!({"type": nyxid_type, "text": null, "attachments": items});
    envelope
}

fn image() -> Envelope {
    let item = resource(0, "image_key", IMAGE_KEY, json!({}));
    lark(
        "image",
        json!({ "image_key": IMAGE_KEY }),
        "image",
        vec![item],
    )
}

/// A file, audio or media message with its raw `content` and envelope `facts`.
fn keyed(message_type: &str, content: Value, nyxid_type: &str, facts: Value) -> Envelope {
    let mut raw = content;
    raw["file_key"] = FILE_KEY.into();
    let item = resource(0, "file_key", FILE_KEY, facts);
    lark(message_type, raw, nyxid_type, vec![item])
}

fn file(name: &str) -> Envelope {
    let facts = json!({ "filename": name });
    keyed("file", json!({ "file_name": name }), "file", facts)
}

fn audio(duration: Value) -> Envelope {
    keyed("audio", json!({ "duration": duration }), "audio", json!({}))
}

/// NyxID d102570afaab's Lark adapter emits both `media` and `video` as video
/// attachments with a file resource URL; its deployed blob also matches main.
fn video(platform: &str) -> Envelope {
    let mut envelope = Envelope::on(platform);
    envelope.lark_message()["message_type"] = "video".into();
    envelope.lark_message()["content"] = json!({"file_key": FILE_KEY,
        "image_key": "img_v3_cover", "file_name": "clip.mp4", "file_size": 51200,
        "duration": 2500})
    .to_string()
    .into();
    let host = if platform == "feishu" {
        "open.feishu.cn"
    } else {
        "open.larksuite.com"
    };
    let item = attachment(
        0,
        "video",
        &format!(
            "https://{host}/open-apis/im/v1/messages/om_message/resources/{FILE_KEY}?type=file"
        ),
        json!({"file_key": FILE_KEY, "platform_message_id": "om_message",
            "filename": "clip.mp4", "size_bytes": 51200}),
    );
    envelope.payload["content"] = json!({"type": "video", "text": null, "attachments": [item]});
    envelope
}

fn sticker() -> Envelope {
    lark(
        "sticker",
        json!({ "file_key": FILE_KEY }),
        "unknown",
        Vec::new(),
    )
}

pub fn post() -> Envelope {
    let content = json!({"title": "", "content": [[{"tag": "text", "text": "hello"}]]});
    lark("post", content, "unknown", Vec::new())
}

pub fn scoped_mention(value: &mut Value) {
    value["sources"][0]["admission"] = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention"});
}

pub async fn media_fixture() -> Fixture {
    Fixture::on_with_media("lark", |_| {}, Default::default()).await
}

pub async fn accept(fixture: &Fixture, envelope: &Envelope, case: &str) {
    let response = fixture.send(envelope).await;
    assert_eq!(response.status(), 202, "{case}");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["status"], "accepted", "{case}: {body}");
}

pub async fn refusal(fixture: &Fixture, envelope: &Envelope, reason: &str) {
    let response = fixture.send(envelope).await;
    assert_eq!(response.status(), 202, "{reason}");
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"status": "refused", "reason": reason})
    );
}

/// The text and attachments of the last launched message.
pub fn last(fixture: &Fixture) -> (String, Vec<Attachment>) {
    let requests = fixture.recorder.requests.lock().unwrap();
    match &requests.last().unwrap().activity.kind {
        ActivityKind::Message {
            text, attachments, ..
        } => (text.clone(), attachments.clone()),
        other => panic!("expected a message, got {other:?}"),
    }
}

/// The provider input the last launched turn renders.
pub fn rendered(fixture: &Fixture) -> String {
    let requests = fixture.recorder.requests.lock().unwrap();
    let request = requests.last().unwrap();
    let audience = Audience {
        tools: false,
        downloads: false,
    };
    let input = render(
        &request.activity,
        &request.channel.definition,
        &RenderMedia::empty(None),
        audience,
    );
    input[0]["content"][0]["text"].as_str().unwrap().to_owned()
}

fn repaired(field: &str) -> String {
    format!("cmaeg_media_facts_repaired_total{{source=\"nyxid_relay\",field=\"{field}\"}}")
}

#[tokio::test]
async fn lark_media_flag_off_is_unchanged() {
    let fixture = Fixture::on("lark", |_| {}, Default::default()).await;
    let video = keyed("media", json!({"duration": 2000}), "video", json!({}));
    for envelope in [
        image(),
        file("report.pdf"),
        audio(json!(2000)),
        video,
        sticker(),
    ] {
        refusal(&fixture, &envelope, "attachment_unsupported").await;
    }
    refusal(&fixture, &post(), "content_unsupported").await;
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lark_image_admitted_with_locator() {
    let fixture = media_fixture().await;
    accept(&fixture, &image(), "image").await;
    let (text, attachments) = last(&fixture);
    assert_eq!(text, "");
    assert_eq!(
        serde_json::to_value(&attachments).unwrap(),
        json!([{"reference": format!("lark:{IMAGE_KEY}"), "kind": "image", "variant": "image",
            "source": {"type": "nyxid_relay", "index": 0}}])
    );
    let input = rendered(&fixture);
    assert!(input.starts_with("[Attachments: 1"), "{input}");
    assert!(input.contains("\n[0] image · "), "{input}");
}

#[tokio::test]
async fn lark_file_audio_media_admitted() {
    let fixture = media_fixture().await;
    accept(&fixture, &file("report.pdf"), "file").await;
    let report = last(&fixture).1.remove(0);
    assert_eq!(
        (report.kind, report.variant),
        (AttachmentKind::File, Some(MediaVariant::File))
    );
    assert_eq!(report.filename.as_deref(), Some("report.pdf"));
    accept(&fixture, &audio(json!(2000)), "audio").await;
    let voice = last(&fixture).1.remove(0);
    assert_eq!(
        (voice.kind, voice.duration_seconds),
        (AttachmentKind::Audio, Some(2))
    );
    let raw = json!({"image_key": "img_v3_cover", "file_name": "clip.mp4", "duration": 2500});
    let video = keyed("media", raw, "video", json!({"filename": "clip.mp4"}));
    accept(&fixture, &video, "media").await;
    let clip = last(&fixture).1.remove(0);
    assert_eq!(
        serde_json::to_value(&clip).unwrap(),
        json!({"reference": format!("lark:{FILE_KEY}"), "kind": "video", "variant": "media",
            "filename": "clip.mp4", "duration_seconds": 3, "source": {"type": "nyxid_relay", "index": 0}})
    );
    assert!(rendered(&fixture).contains("\n[0] video · 0:03 · "));
}

#[tokio::test]
async fn lark_and_feishu_video_matches_deployed_nyxid_shape() {
    for platform in ["lark", "feishu"] {
        let fixture = Fixture::on_with_media(platform, |_| {}, Default::default()).await;
        accept(&fixture, &video(platform), platform).await;
        let (text, attachments) = last(&fixture);
        assert_eq!(text, "");
        assert_eq!(
            serde_json::to_value(attachments).unwrap(),
            json!([{
                "reference": format!("lark:{FILE_KEY}"), "kind": "video", "variant": "media",
                "filename": "clip.mp4", "size_bytes": 51200, "duration_seconds": 3,
                "source": {"type": "nyxid_relay", "index": 0}
            }])
        );
        let mut mismatch = video(platform);
        mismatch.payload["content"]["type"] = "file".into();
        refusal(&fixture, &mismatch, "content_unsupported").await;
        assert_eq!(fixture.recorder.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn lark_and_feishu_video_stays_refused_without_media_gate() {
    for platform in ["lark", "feishu"] {
        let fixture = Fixture::on(platform, |_| {}, Default::default()).await;
        refusal(&fixture, &video(platform), "attachment_unsupported").await;
        assert!(fixture.recorder.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn lark_index_follows_nyxid_position() {
    let fixture = media_fixture().await;
    let mut envelope = image();
    envelope.payload["content"]["attachments"] =
        json!([7, resource(1, "image_key", IMAGE_KEY, json!({}))]);
    accept(&fixture, &envelope, "dropped first item").await;
    let source = serde_json::to_value(&last(&fixture).1[0].source).unwrap();
    assert_eq!(source, json!({"type": "nyxid_relay", "index": 1}));
}

#[test]
fn lark_long_file_name_is_truncated_not_refused() {
    run_isolated("lark_media::lark_long_file_name_is_truncated_not_refused_child");
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn lark_long_file_name_is_truncated_not_refused_child() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let fixture = media_fixture().await;
    let before = counter(&metrics.render(), &repaired("filename"));
    accept(
        &fixture,
        &file(&format!("{}.pdf", "a".repeat(396))),
        "long name",
    )
    .await;
    let name = last(&fixture).1[0].filename.clone().unwrap();
    assert!(name.len() <= 255 && name.ends_with("….pdf"), "{name}");
    assert_eq!(
        counter(&metrics.render(), &repaired("filename")),
        before + 1
    );
}

#[test]
fn lark_huge_duration_is_dropped() {
    run_isolated("lark_media::lark_huge_duration_is_dropped_child");
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn lark_huge_duration_is_dropped_child() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let fixture = media_fixture().await;
    let before = counter(&metrics.render(), &repaired("duration_seconds"));
    accept(&fixture, &audio(json!(90_000_000_000_u64)), "huge duration").await;
    assert_eq!(last(&fixture).1[0].duration_seconds, None);
    let after = counter(&metrics.render(), &repaired("duration_seconds"));
    assert_eq!(after, before + 1);
}

#[tokio::test]
async fn lark_non_integer_duration_is_ignored() {
    let fixture = media_fixture().await;
    for duration in [json!("2000"), json!(2000.5)] {
        accept(&fixture, &audio(duration.clone()), "non-integer").await;
        assert_eq!(last(&fixture).1[0].duration_seconds, None, "{duration}");
    }
}

#[tokio::test]
async fn lark_type_mismatch_is_content_unsupported() {
    let fixture = media_fixture().await;
    let mut kind = image();
    kind.payload["content"]["type"] = "file".into();
    let mut text = image();
    text.payload["content"]["text"] = "a caption".into();
    let mut malformed = image();
    malformed.lark_message()["content"] = "[\"not an object\"]".into();
    for envelope in [kind, text, malformed] {
        refusal(&fixture, &envelope, "content_unsupported").await;
    }
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[test]
fn lark_unmatched_key_has_no_source() {
    run_isolated("lark_media::lark_unmatched_key_has_no_source_child");
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn lark_unmatched_key_has_no_source_child() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let series =
        |outcome: &str| format!("cmaeg_media_relay_correlation_total{{outcome=\"{outcome}\"}}");
    let fixture = media_fixture().await;
    let mut envelope = image();
    envelope.payload["content"]["attachments"][0]["image_key"] = "img_v3_another".into();
    let before = metrics.render();
    accept(&fixture, &envelope, "unmatched").await;
    let after = metrics.render();
    let attachment = last(&fixture).1.remove(0);
    assert_eq!(attachment.source, None);
    assert_eq!(attachment.reference, format!("lark:{IMAGE_KEY}"));
    for (outcome, increase) in [("unmatched", 1), ("matched", 0)] {
        let series = series(outcome);
        assert_eq!(
            counter(&after, &series),
            counter(&before, &series) + increase
        );
    }
    assert!(rendered(&fixture).contains("not retrievable through this channel"));
}

#[test]
fn lark_invalid_key_degrades() {
    run_isolated("lark_media::lark_invalid_key_degrades_child");
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn lark_invalid_key_degrades_child() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let series = "cmaeg_media_items_total{source=\"nyxid_relay\",kind=\"image\",outcome=\"invalid_locator\"}";
    let fixture = media_fixture().await;
    for key in [
        "img v3".to_owned(),
        "k".repeat(1_025),
        "img_v3_é".to_owned(),
    ] {
        let mut envelope = image();
        envelope.lark_message()["content"] = json!({ "image_key": key }).to_string().into();
        let before = counter(&metrics.render(), series);
        accept(&fixture, &envelope, "invalid key").await;
        let attachment = last(&fixture).1.remove(0);
        assert_eq!(attachment.reference, "lark:item-0");
        assert_eq!(attachment.source, None);
        assert_eq!(counter(&metrics.render(), series), before + 1);
    }
}

#[tokio::test]
async fn lark_sticker_still_refused() {
    let fixture = media_fixture().await;
    refusal(&fixture, &sticker(), "attachment_unsupported").await;
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lark_group_media_under_mention_policy_is_not_addressed() {
    let fixture = Fixture::on_with_media("lark", scoped_mention, Default::default()).await;
    let mut envelope = image();
    envelope.group();
    refusal(&fixture, &envelope, "group_not_addressed").await;
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lark_text_with_envelope_attachment_still_refused() {
    let fixture = media_fixture().await;
    let mut envelope = Envelope::on("lark");
    envelope.payload["content"]["attachments"] =
        json!([resource(0, "image_key", IMAGE_KEY, json!({}))]);
    refusal(&fixture, &envelope, "attachment_unsupported").await;
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}
