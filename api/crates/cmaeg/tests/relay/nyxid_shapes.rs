//! Callback envelopes as serialized by NyxID `f3dc2be`.
//! The relay supplies attachment ordinals and Telegram captions and file IDs.
//! Lark resources carry their message and file keys in provider URLs.
use super::support::*;
use cmaeg_core::activity::ActivityKind;
use serde_json::{json, Value};

pub const TELEGRAM_FILE_ID: &str = "AgACAgIAAxkBAAIBX2Zx-test_file_id";
const LARK: &str = "https://open.larksuite.com/open-apis/im/v1/messages/om_message/resources";
const REFUSED: &str = "cmaeg_refusals_total{reason=\"attachment_unsupported\"}";

/// NyxID's `CallbackAttachment` at original list position `index`.
pub fn attachment(index: usize, content_type: &str, url: &str, facts: Value) -> Value {
    let mut value = json!({
        "download_url": format!("{ISSUER}/api/v1/channel-relay/messages/message-1/attachments/{index}"),
        "content_type": content_type,
        "url": url,
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(facts.as_object().unwrap().clone());
    value
}

/// A Telegram update whose message carries the raw `media` fields instead of text.
pub fn telegram(
    media: Value,
    kind: &str,
    caption: Option<&str>,
    attachments: Vec<Value>,
) -> Envelope {
    let mut envelope = Envelope::new();
    envelope.payload["sender"]["display_name"] = "Mira Fixture".into();
    let message = envelope.payload["raw_platform_data"]["message"]
        .as_object_mut()
        .unwrap();
    message.remove("text");
    message.extend(media.as_object().unwrap().clone());
    message.insert(
        "from".into(),
        json!({"id": 7, "is_bot": false, "first_name": "Mira", "last_name": "Fixture"}),
    );
    envelope.payload["content"] = json!({ "type": kind, "attachments": attachments });
    if let Some(caption) = caption {
        envelope.payload["raw_platform_data"]["message"]["caption"] = caption.into();
        envelope.payload["content"]["text"] = caption.into();
    }
    envelope
}
pub fn file(extra: Value) -> Value {
    let mut value = json!({"file_id": TELEGRAM_FILE_ID, "file_unique_id": "AgADfixture"});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    value
}

pub fn telegram_photo() -> Envelope {
    let small = json!({"file_id": "AgACAgIAAxkBAAIBX2Zx-small", "file_unique_id": "AQADs", "width": 90, "height": 90, "file_size": 1200});
    let large = file(json!({"width": 1280, "height": 960, "file_size": 81234}));
    telegram(
        json!({ "photo": [small, large] }),
        "image",
        Some("look at this"),
        vec![attachment(
            0,
            "image",
            TELEGRAM_FILE_ID,
            json!({"size_bytes": 81234}),
        )],
    )
}
pub fn telegram_document() -> Envelope {
    let facts =
        json!({"filename": "report.pdf", "mime_type": "application/pdf", "size_bytes": 20480});
    telegram(
        json!({"document": file(json!({"file_name": "report.pdf", "mime_type": "application/pdf", "file_size": 20480}))}),
        "file",
        None,
        vec![attachment(0, "file", TELEGRAM_FILE_ID, facts)],
    )
}
pub fn telegram_shapes() -> Vec<(&'static str, Envelope)> {
    let clip = json!({"file_name": "fun.gif.mp4", "mime_type": "video/mp4", "size_bytes": 51200});
    vec![
        ("photo with caption", telegram_photo()),
        (
            "voice",
            telegram(
                json!({"voice": file(json!({"duration": 3, "mime_type": "audio/ogg", "file_size": 5120}))}),
                "audio",
                None,
                vec![attachment(
                    0,
                    "audio",
                    TELEGRAM_FILE_ID,
                    json!({"mime_type": "audio/ogg", "size_bytes": 5120}),
                )],
            ),
        ),
        (
            "audio",
            telegram(
                json!({"audio": file(json!({"duration": 180, "file_name": "song.mp3", "mime_type": "audio/mpeg", "file_size": 4_200_000}))}),
                "audio",
                None,
                vec![attachment(
                    0,
                    "audio",
                    TELEGRAM_FILE_ID,
                    json!({"filename": "song.mp3", "mime_type": "audio/mpeg", "size_bytes": 4_200_000}),
                )],
            ),
        ),
        (
            "video",
            telegram(
                json!({"video": file(json!({"width": 640, "height": 360, "duration": 9, "file_name": "clip.mp4", "mime_type": "video/mp4", "file_size": 9_000_000}))}),
                "video",
                None,
                vec![attachment(
                    0,
                    "video",
                    TELEGRAM_FILE_ID,
                    json!({"filename": "clip.mp4", "mime_type": "video/mp4", "size_bytes": 9_000_000}),
                )],
            ),
        ),
        (
            "video_note",
            telegram(
                json!({"video_note": file(json!({"length": 240, "duration": 5, "file_size": 700_000}))}),
                "video",
                None,
                vec![attachment(
                    0,
                    "video",
                    TELEGRAM_FILE_ID,
                    json!({"size_bytes": 700_000}),
                )],
            ),
        ),
        ("document", telegram_document()),
        (
            "animation sharing its document's file_id",
            telegram(
                json!({
                    "animation": file(json!({"width": 320, "height": 240, "duration": 2, "file_name": "fun.gif.mp4", "mime_type": "video/mp4", "file_size": 51200})),
                    "document": file(json!({"file_name": "fun.gif.mp4", "mime_type": "video/mp4", "file_size": 51200})),
                }),
                "file",
                None,
                vec![
                    attachment(0, "file", TELEGRAM_FILE_ID, clip.clone()),
                    attachment(1, "video", TELEGRAM_FILE_ID, clip),
                ],
            ),
        ),
        (
            "static sticker",
            telegram(
                json!({"sticker": file(json!({"type": "regular", "width": 512, "height": 512, "is_animated": false, "is_video": false, "file_size": 30000}))}),
                "image",
                None,
                vec![attachment(
                    0,
                    "image",
                    TELEGRAM_FILE_ID,
                    json!({"size_bytes": 30000}),
                )],
            ),
        ),
    ]
}

/// A Lark `im.message.receive_v1` media message and NyxID's single attachment.
fn lark(
    message_type: &str,
    content: Value,
    kind: &str,
    key: (&str, &str),
    facts: Value,
) -> Envelope {
    let mut envelope = Envelope::on("lark");
    envelope.lark_message()["message_type"] = message_type.into();
    envelope.lark_message()["content"] = content.to_string().into();
    let url = format!("{LARK}/{}?type={}", key.0, key.1);
    let mut facts = facts;
    facts["platform_message_id"] = "om_message".into();
    envelope.payload["content"] =
        json!({ "type": kind, "attachments": [attachment(0, kind, &url, facts)] });
    envelope
}
fn lark_shapes() -> Vec<(&'static str, Envelope)> {
    vec![
        (
            "image",
            lark(
                "image",
                json!({"image_key": "img_v3_photo"}),
                "image",
                ("img_v3_photo", "image"),
                json!({"image_key": "img_v3_photo"}),
            ),
        ),
        (
            "file",
            lark(
                "file",
                json!({"file_key": "file_v3_report", "file_name": "report.pdf"}),
                "file",
                ("file_v3_report", "file"),
                json!({"file_key": "file_v3_report", "filename": "report.pdf"}),
            ),
        ),
        (
            "audio",
            lark(
                "audio",
                json!({"file_key": "file_v3_audio", "duration": 3000}),
                "audio",
                ("file_v3_audio", "file"),
                json!({"file_key": "file_v3_audio"}),
            ),
        ),
        (
            "media",
            lark(
                "media",
                json!({"file_key": "file_v3_media", "image_key": "img_v3_cover", "file_name": "clip.mp4", "duration": 2000}),
                "video",
                ("file_v3_media", "file"),
                json!({"file_key": "file_v3_media", "filename": "clip.mp4"}),
            ),
        ),
    ]
}

pub fn repairs(field: &str) -> String {
    format!("cmaeg_relay_envelope_repairs_total{{field=\"{field}\"}}")
}
/// A counter sample from a Prometheus render; an unrecorded series is zero.
pub fn counter(rendered: &str, series: &str) -> u64 {
    rendered
        .lines()
        .find_map(|line| line.strip_prefix(series)?.strip_prefix(' '))
        .map_or(0, |value| value.trim().parse().unwrap())
}
pub async fn refused_media(fixture: &Fixture, envelope: &Envelope, shape: &str) {
    let metrics = cmaeg::telemetry::install().unwrap();
    let before = counter(&metrics.render(), REFUSED);
    let response = fixture.send(envelope).await;
    assert_eq!(response.status(), 202, "{shape}");
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"status": "refused", "reason": "attachment_unsupported"}),
        "{shape}"
    );
    assert_eq!(counter(&metrics.render(), REFUSED), before + 1, "{shape}");
    assert!(
        fixture.recorder.requests.lock().unwrap().is_empty(),
        "{shape}"
    );
}
/// The same attachment list on a plain text message, so only the presence check
/// can refuse it: a malformed list must never become a text-only turn.
fn on_text(shape: &Envelope) -> Envelope {
    let mut envelope = Envelope::new();
    envelope.payload["content"]["attachments"] = shape.payload["content"]["attachments"].clone();
    envelope
}

#[test]
fn nyxid_telegram_media_is_refused_as_unsupported_not_rejected() {
    run_isolated("nyxid_shapes::nyxid_telegram_media_is_refused_as_unsupported_not_rejected_child");
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn nyxid_telegram_media_is_refused_as_unsupported_not_rejected_child() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    for (shape, envelope) in telegram_shapes() {
        let before = counter(&metrics.render(), &repairs("url"));
        refused_media(&fixture, &envelope, shape).await;
        // A bare file_id is an opaque handle, not a malformed URL.
        assert_eq!(
            counter(&metrics.render(), &repairs("url")),
            before,
            "{shape}"
        );
    }
}

#[test]
fn nyxid_lark_media_is_refused_as_unsupported_not_rejected() {
    run_isolated("nyxid_shapes::nyxid_lark_media_is_refused_as_unsupported_not_rejected_child");
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn nyxid_lark_media_is_refused_as_unsupported_not_rejected_child() {
    let fixture = Fixture::on("lark", |_| {}, Default::default()).await;
    for (shape, envelope) in lark_shapes() {
        refused_media(&fixture, &envelope, shape).await;
    }
}

#[test]
fn sender_controlled_attachment_facts_never_reject_the_callback() {
    run_isolated(
        "nyxid_shapes::sender_controlled_attachment_facts_never_reject_the_callback_child",
    );
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn sender_controlled_attachment_facts_never_reject_the_callback_child() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let edit = |change: &dyn Fn(&mut Value)| {
        let mut envelope = telegram_document();
        change(&mut envelope.payload["content"]["attachments"]);
        envelope
    };
    let many: Vec<_> = (0..33)
        .map(|index| attachment(index, "file", TELEGRAM_FILE_ID, json!({})))
        .collect();
    let shapes = [
        (
            "600-byte CJK filename",
            edit(&|list| list[0]["filename"] = "文".repeat(200).into()),
        ),
        (
            "filename with U+0007",
            edit(&|list| list[0]["filename"] = "report\u{7}.pdf".into()),
        ),
        (
            "empty filename",
            edit(&|list| list[0]["filename"] = "".into()),
        ),
        (
            "300-byte mime_type",
            edit(&|list| list[0]["mime_type"] = "m".repeat(300).into()),
        ),
        (
            "non-object item first",
            edit(&|list| *list = json!([7, attachment(1, "file", TELEGRAM_FILE_ID, json!({}))])),
        ),
        ("33 items", edit(&|list| *list = Value::Array(many.clone()))),
        ("numeric url", edit(&|list| list[0]["url"] = 12345.into())),
        (
            "string size_bytes",
            edit(&|list| list[0]["size_bytes"] = "20480".into()),
        ),
        (
            "pre-#1583 without download_url",
            edit(&|list| {
                list[0].as_object_mut().unwrap().remove("download_url");
            }),
        ),
        ("object container", edit(&|list| *list = json!({}))),
        ("number container", edit(&|list| *list = json!(7))),
        (
            "url with U+0007",
            edit(&|list| list[0]["url"] = "AgACAgIA\u{7}".into()),
        ),
    ];
    for (shape, envelope) in &shapes {
        refused_media(&fixture, envelope, shape).await;
        refused_media(&fixture, &on_text(envelope), shape).await;
    }
}

#[tokio::test]
async fn a_null_attachment_list_is_a_text_message() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let mut envelope = Envelope::new();
    envelope.payload["content"]["attachments"] = Value::Null;
    let response = fixture.send(&envelope).await;
    assert_eq!(response.status(), 202);
    assert_eq!(
        response.json::<Value>().await.unwrap()["status"],
        "accepted"
    );
    let requests = fixture.recorder.requests.lock().unwrap();
    assert!(
        matches!(&requests[0].activity.kind, ActivityKind::Message { text, attachments, .. } if text == "hello" && attachments.is_empty())
    );
}

#[test]
fn repaired_attachment_facts_are_counted_by_field() {
    run_isolated("nyxid_shapes::repaired_attachment_facts_are_counted_by_field_child");
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn repaired_attachment_facts_are_counted_by_field_child() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let mut envelope = telegram_document();
    envelope.payload["content"]["attachments"][0]["filename"] = "文".repeat(200).into();
    envelope.payload["content"]["attachments"][0]["size_bytes"] = "20480".into();
    let before = metrics.render();
    refused_media(&fixture, &envelope, "repaired").await;
    let after = metrics.render();
    for (field, increase) in [("filename", 1), ("size_bytes", 1), ("url", 0)] {
        assert_eq!(
            counter(&after, &repairs(field)),
            counter(&before, &repairs(field)) + increase,
            "{field}"
        );
    }
}

#[tokio::test]
async fn dormant_nyxid_thread_and_activity_fields_are_accepted_and_ignored() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let plain = Envelope::new();
    let mut dormant = Envelope::new();
    dormant.payload["timestamp"] = plain.payload["timestamp"].clone();
    dormant.payload["thread_context"] = json!({"root_id": "x"});
    dormant.payload["activity"] = json!({"kind": "message"});
    for envelope in [&plain, &dormant] {
        let response = fixture.send(envelope).await;
        assert_eq!(response.status(), 202);
        assert_eq!(
            response.json::<Value>().await.unwrap()["status"],
            "accepted"
        );
    }
    {
        let requests = fixture.recorder.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].activity, requests[0].activity);
    }
    let mut unknown = Envelope::new();
    unknown.payload["surprise"] = json!({"root_id": "x"});
    problem(fixture.send(&unknown).await, 401, "payload_invalid").await;
    assert_eq!(fixture.recorder.requests.lock().unwrap().len(), 2);
}

/// A text callback whose display name NyxID built from `first_name` and, when
/// present, `" " + last_name`.
fn named(first: &str, last: Option<&str>) -> Envelope {
    let mut envelope = Envelope::new();
    let from = &mut envelope.payload["raw_platform_data"]["message"]["from"];
    from["first_name"] = first.into();
    let display = match last {
        Some(last) => {
            from["last_name"] = last.into();
            format!("{first} {last}")
        }
        None => first.to_owned(),
    };
    envelope.payload["sender"]["display_name"] = display.into();
    envelope
}

#[test]
fn oversized_and_control_only_display_names_are_repaired_not_rejected() {
    run_isolated(
        "nyxid_shapes::oversized_and_control_only_display_names_are_repaired_not_rejected_child",
    );
}

#[tokio::test]
#[ignore = "run by the enclosing telemetry test in its own process"]
async fn oversized_and_control_only_display_names_are_repaired_not_rejected_child() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let cjk = named(&"名".repeat(64), Some(&"姓".repeat(64)));
    let original = cjk.payload["sender"]["display_name"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(original.len(), 385);
    let before = counter(&metrics.render(), &repairs("sender_display_name"));
    for envelope in [&cjk, &named("\u{7}\u{1b}", None)] {
        let response = fixture.send(envelope).await;
        assert_eq!(response.status(), 202);
        assert_eq!(
            response.json::<Value>().await.unwrap()["status"],
            "accepted"
        );
    }
    assert_eq!(
        counter(&metrics.render(), &repairs("sender_display_name")),
        before + 2
    );
    let requests = fixture.recorder.requests.lock().unwrap();
    let truncated = requests[0].activity.actor.display_name.as_deref().unwrap();
    assert!(truncated.len() <= 256);
    assert!(original.starts_with(truncated));
    assert_eq!(requests[1].activity.actor.display_name, None);
}
