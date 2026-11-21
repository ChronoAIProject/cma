//! Extractor unit tests: the extraction rules one by one, and a seeded
//! invariant that no raw Lark content and no envelope can produce an attachment
//! the validator refuses.
use super::*;
use crate::ingress::relay::seeded_rng::Rng;
use cmaeg_core::{
    activity::{validate_attachments, SourceKind},
    channel::Platform,
};
use serde_json::{json, Map};

fn envelope(items: Value) -> RelayAttachments {
    serde_json::from_value(items).unwrap()
}

/// A NyxID envelope entry naming `key` in `field`, with `facts` added.
fn entry(field: &str, key: Value, facts: Value) -> Value {
    let mut value = json!({"content_type": "file", "url": "https://open.larksuite.com/open-apis/im/v1/messages/om_1/resources/r"});
    value[field] = key;
    value
        .as_object_mut()
        .unwrap()
        .extend(facts.as_object().unwrap().clone());
    value
}

fn media(message_type: &str, content: Value, items: Value) -> LarkMedia {
    extract(message_type, &content, &envelope(items)).unwrap()
}

#[test]
fn only_image_file_audio_media_and_video_messages_are_media() {
    let content = json!({"image_key": "img_v3_k", "file_key": "file_v3_k"});
    for other in ["text", "post", "sticker", "share_chat", "IMAGE", ""] {
        assert!(extract(other, &content, &RelayAttachments::default()).is_none());
    }
    for (message_type, kind, variant, nyxid_type) in [
        ("image", AttachmentKind::Image, MediaVariant::Image, "image"),
        ("file", AttachmentKind::File, MediaVariant::File, "file"),
        ("audio", AttachmentKind::Audio, MediaVariant::Audio, "audio"),
        ("media", AttachmentKind::Video, MediaVariant::Media, "video"),
        ("video", AttachmentKind::Video, MediaVariant::Media, "video"),
    ] {
        let media = media(message_type, content.clone(), json!([]));
        assert_eq!(media.attachment.kind, kind);
        assert_eq!(media.attachment.variant, Some(variant));
        assert_eq!(media.nyxid_type, nyxid_type);
        let key = if message_type == "image" {
            "img_v3_k"
        } else {
            "file_v3_k"
        };
        assert_eq!(media.attachment.reference, format!("lark:{key}"));
    }
}

#[test]
fn keys_are_ascii_graphic_strings_of_at_most_1024_bytes() {
    for valid in [
        json!("img_v3_0abc-x"),
        json!("!~"),
        json!("k".repeat(1_024)),
    ] {
        assert_eq!(lark_key(Some(&valid)), valid.as_str());
    }
    for invalid in [
        Value::Null,
        json!(7),
        json!(["img_v3_k"]),
        json!(""),
        json!("k".repeat(1_025)),
        json!("has space"),
        json!("tab\tkey"),
        json!("bell\u{7}"),
        json!("img_é"),
        json!("文"),
    ] {
        assert_eq!(lark_key(Some(&invalid)), None, "{invalid}");
    }
    assert_eq!(lark_key(None), None);
}

#[test]
fn durations_round_milliseconds_up_and_ignore_non_integers() {
    for (milliseconds, seconds) in [
        (json!(0), 0),
        (json!(1), 1),
        (json!(1_000), 1),
        (json!(1_001), 2),
        (json!(2_000), 2),
        (json!(u64::MAX), u32::MAX),
    ] {
        assert_eq!(duration_seconds(Some(&milliseconds)), Some(seconds));
    }
    for invalid in [
        json!(-1),
        json!(2000.5),
        json!(2000.0),
        json!("2000"),
        Value::Null,
    ] {
        assert_eq!(duration_seconds(Some(&invalid)), None, "{invalid}");
    }
    assert_eq!(duration_seconds(None), None);
    // Over 86400 seconds the fact is dropped by normalization, never clamped.
    let long = media(
        "audio",
        json!({"file_key": "file_v3_a", "duration": 90_000_000_000_u64}),
        json!([]),
    );
    assert_eq!(long.attachment.duration_seconds, None);
    assert_eq!(
        long.repairs.fields().collect::<Vec<_>>(),
        ["duration_seconds"]
    );
    let image = media(
        "image",
        json!({"image_key": "img_v3_k", "duration": 2_000}),
        json!([]),
    );
    assert_eq!(image.attachment.duration_seconds, None);
}

#[test]
fn filenames_prefer_the_raw_name_and_are_normalized() {
    let items = json!([entry(
        "file_key",
        json!("file_v3_r"),
        json!({"filename": "envelope.pdf"})
    )]);
    let named = |message_type: &str, content: Value| media(message_type, content, items.clone());
    let raw = named(
        "file",
        json!({"file_key": "file_v3_r", "file_name": "raw.pdf"}),
    );
    assert_eq!(raw.attachment.filename.as_deref(), Some("raw.pdf"));
    for content in [
        json!({"file_key": "file_v3_r"}),
        json!({"file_key": "file_v3_r", "file_name": 7}),
    ] {
        for message_type in ["media", "video"] {
            let fallback = named(message_type, content.clone()).attachment;
            assert_eq!(fallback.filename.as_deref(), Some("envelope.pdf"));
        }
    }
    let long = named(
        "file",
        json!({"file_key": "file_v3_r", "file_name": format!("{}.pdf", "a".repeat(396))}),
    );
    let name = long.attachment.filename.as_deref().unwrap();
    assert!(name.len() <= 255 && name.ends_with("….pdf"), "{name}");
    assert_eq!(long.repairs.fields().collect::<Vec<_>>(), ["filename"]);
    let disguised = named(
        "file",
        json!({"file_key": "file_v3_r", "file_name": "a\u{202E}fdp.exe\u{7}"}),
    );
    assert_eq!(disguised.attachment.filename.as_deref(), Some("afdp.exe"));
    let empty = named(
        "file",
        json!({"file_key": "file_v3_r", "file_name": "\u{7}"}),
    );
    assert_eq!(empty.attachment.filename, None);
    // Images and voice messages declare no name, so none is taken from the envelope.
    assert_eq!(
        named("audio", json!({"file_key": "file_v3_r"}))
            .attachment
            .filename,
        None
    );
}

#[test]
fn correlation_takes_the_first_entry_with_the_key_at_its_original_index() {
    let items = json!([
        7,
        entry("file_key", json!("img_v3_p"), json!({})),
        entry("image_key", json!("img_v3_other"), json!({})),
        entry(
            "image_key",
            json!("img_v3_p"),
            json!({"mime_type": "Image/PNG; q=1", "size_bytes": 2048})
        ),
        entry("image_key", json!("img_v3_p"), json!({"size_bytes": 1})),
    ]);
    let image = media("image", json!({"image_key": "img_v3_p"}), items).attachment;
    assert_eq!(
        image.source,
        Some(AttachmentSource::NyxidRelay { index: 3 })
    );
    assert_eq!(image.mime_type.as_deref(), Some("image/png"));
    assert_eq!(image.size_bytes, Some(2048));
    // A media message's cover is listed under image_key and never matches.
    let items = json!([
        entry("image_key", json!("file_v3_m"), json!({})),
        entry("file_key", json!("file_v3_m"), json!({"size_bytes": 9})),
    ]);
    let video = media(
        "media",
        json!({"file_key": "file_v3_m", "image_key": "file_v3_m"}),
        items,
    )
    .attachment;
    assert_eq!(
        video.source,
        Some(AttachmentSource::NyxidRelay { index: 1 })
    );
    assert_eq!(video.size_bytes, Some(9));
    // An envelope key the lenient parser dropped, or a different key, matches nothing.
    for items in [
        json!([entry("file_key", json!(12345), json!({"size_bytes": 9}))]),
        json!([entry("file_key", json!("12346"), json!({"size_bytes": 9}))]),
        json!([]),
    ] {
        let file = media("file", json!({"file_key": "12345"}), items).attachment;
        assert_eq!((file.source, file.size_bytes), (None, None));
        assert_eq!(file.reference, "lark:12345");
    }
}

#[test]
fn unusable_keys_degrade_to_an_unlocated_item_reference() {
    for key in [
        json!("has space"),
        json!("k".repeat(1_025)),
        json!("img_é"),
        json!(""),
        json!(7),
    ] {
        let items = json!([entry("image_key", key.clone(), json!({}))]);
        let image = media("image", json!({"image_key": key}), items).attachment;
        assert_eq!(
            (image.reference.as_str(), image.source),
            ("lark:item-0", None)
        );
    }
    let missing = media("file", json!({"file_name": "a.pdf"}), json!([])).attachment;
    assert_eq!(missing.reference, "lark:item-0");
    assert_eq!(missing.filename.as_deref(), Some("a.pdf"));
}

fn text(rng: &mut Rng) -> Value {
    const PIECES: [&str; 14] = [
        "a", "Z", "9", "_", ".", " ", "文", "😀", "\u{0}", "\u{85}", "\u{202E}", "\t", "\n", ".pdf",
    ];
    let length = rng.pick(&[0, 1, 5, 40, 255, 256, 400, 1_024, 1_025]);
    let mut value = String::new();
    while value.len() < length {
        value.push_str(rng.pick(&PIECES));
    }
    Value::String(value)
}

/// A key from the shared pool, so raw content and envelope sometimes agree.
fn key(rng: &mut Rng, pool: &[Value]) -> Value {
    match rng.below(4) {
        0 => text(rng),
        1 => rng.pick(&[json!(7), Value::Null, json!(["img_v3_a"])]),
        _ => rng.pick(pool),
    }
}

fn content(rng: &mut Rng, pool: &[Value]) -> Value {
    let mut object = Map::new();
    for field in ["image_key", "file_key", "file_name", "duration"] {
        let value = match (rng.below(3), field) {
            (0, _) => continue,
            (1, "file_name") => json!(42),
            (_, "file_name") => text(rng),
            (_, "duration") => rng.pick(&[
                json!(0),
                json!(999),
                json!(86_400_000),
                json!(86_400_001),
                json!(u64::MAX),
                json!(-1),
                json!(1.5),
                json!("12"),
                Value::Null,
            ]),
            _ => key(rng, pool),
        };
        object.insert(field.into(), value);
    }
    Value::Object(object)
}

fn entries(rng: &mut Rng, pool: &[Value]) -> RelayAttachments {
    let items = (0..rng.below(40))
        .map(|_| {
            if rng.below(6) == 0 {
                return json!(7);
            }
            let field = rng.pick(&["image_key", "file_key"]);
            let mime = match rng.below(4) {
                0 => text(rng),
                _ => rng.pick(&[
                    json!("audio/ogg; codecs=opus"),
                    json!("Image/PNG"),
                    json!("video/"),
                ]),
            };
            let size = rng.pick(&[json!(0), json!(i64::MAX), json!(-1)]);
            let facts = json!({"filename": text(rng), "mime_type": mime, "size_bytes": size});
            entry(field, key(rng, pool), facts)
        })
        .collect();
    envelope(Value::Array(items))
}

#[test]
fn extractor_output_always_validates() {
    let source = SourceKind::NyxidRelay {
        platform: Platform::Lark,
        route_id: "route-1".into(),
    };
    let pool = [
        json!("img_v3_a"),
        json!("file_v3_b"),
        json!("k".repeat(1_024)),
    ];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut located, mut keyed) = (0, 0);
    for case in 0..2_000 {
        let message_type = rng.pick(&["image", "file", "audio", "media", "video"]);
        let content = content(&mut rng, &pool);
        let list = entries(&mut rng, &pool);
        let media = extract(message_type, &content, &list).unwrap();
        located += usize::from(media.attachment.source.is_some());
        keyed += usize::from(media.attachment.reference != "lark:item-0");
        if let Err(error) = validate_attachments(
            std::slice::from_ref(&media.attachment),
            &source,
            "/kind/attachments",
        ) {
            panic!("case {case}: {error}\n{content}");
        }
    }
    // The generator must exercise every locator outcome.
    assert!(keyed > 600 && keyed < 1_900, "{keyed} keyed");
    assert!(
        located > 200 && located < keyed,
        "{located} of {keyed} located"
    );
}
