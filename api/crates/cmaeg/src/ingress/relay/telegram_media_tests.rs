//! Extractor unit tests: targeted facts plus a seeded invariant that no raw
//! Telegram media can produce an attachment the activity validator refuses.
use super::*;
use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, ConversationKind,
        SourceKind,
    },
    channel::Platform,
    ids::EventId,
};
use serde_json::{json, Map};

const FILE_ID: &str = "BAACAgIAAxkBAAIBfixture";

fn relay_list(items: Value) -> RelayAttachments {
    serde_json::from_value(items).unwrap()
}

fn native(message: Value) -> TelegramMedia {
    extract(&message, Transport::Native, "telegram").unwrap()
}

fn relay(message: Value, items: Value) -> TelegramMedia {
    extract(
        &message,
        Transport::Relay(&relay_list(items)),
        "nyxid_relay",
    )
    .unwrap()
}

#[test]
fn a_message_without_media_fields_is_not_media() {
    let message = json!({"text": "hello", "caption": "ignored"});
    assert!(extract(&message, Transport::Native, "telegram").is_none());
}

#[test]
fn an_animation_skips_its_duplicate_document_and_correlates_once() {
    let clip = json!({"file_id": FILE_ID, "file_unique_id": "AgADclip", "width": 320, "height": 240, "duration": 2, "mime_type": "video/mp4"});
    let message = json!({"animation": clip, "document": {"file_id": FILE_ID, "file_unique_id": "AgADclip", "mime_type": "video/mp4"}});
    let media = relay(
        message,
        json!([{"content_type": "file", "url": FILE_ID}, {"content_type": "video", "url": FILE_ID}]),
    );
    assert_eq!(media.attachments.len(), 1);
    let animation = &media.attachments[0];
    assert_eq!(animation.variant, Some(MediaVariant::Animation));
    assert_eq!(animation.kind, AttachmentKind::Video);
    assert_eq!(animation.reference, "telegram:AgADclip");
    assert_eq!(
        animation.source,
        Some(AttachmentSource::NyxidRelay { index: 0 })
    );
    assert_eq!(media.nyxid_type, "file");
}

#[test]
fn relay_correlation_uses_each_envelope_item_once_and_keeps_its_original_index() {
    // A photo and a document with the same file_id never happen on Telegram, but
    // correlation must still give distinct items distinct envelope positions.
    let message = json!({
        "photo": [{"file_id": FILE_ID, "file_unique_id": "AgADphoto", "width": 90, "height": 90}],
        "document": {"file_id": FILE_ID, "file_unique_id": "AgADdoc"},
        "voice": {"file_id": "other-file", "file_unique_id": "AgADvoice"},
    });
    let media = relay(
        message,
        json!([7, {"content_type": "image", "url": FILE_ID}, {"content_type": "file", "url": FILE_ID}]),
    );
    let indexes: Vec<_> = media
        .attachments
        .iter()
        .map(|attachment| attachment.source.clone())
        .collect();
    assert_eq!(
        indexes,
        [
            Some(AttachmentSource::NyxidRelay { index: 1 }),
            Some(AttachmentSource::NyxidRelay { index: 2 }),
            None,
        ]
    );
    assert!(media
        .attachments
        .iter()
        .all(|attachment| attachment.thumbnail.is_none()));
}

#[test]
fn documents_take_their_kind_from_the_normalized_mime_type() {
    for (mime, kind, normalized) in [
        ("IMAGE/PNG", AttachmentKind::Image, Some("image/png")),
        (
            "audio/ogg; codecs=opus",
            AttachmentKind::Audio,
            Some("audio/ogg"),
        ),
        ("video/mp4", AttachmentKind::Video, Some("video/mp4")),
        (
            "text/plain; charset=utf-8",
            AttachmentKind::File,
            Some("text/plain"),
        ),
        ("not a mime", AttachmentKind::File, None),
    ] {
        let media = native(
            json!({"document": {"file_id": FILE_ID, "file_unique_id": "AgADdoc", "mime_type": mime}}),
        );
        assert_eq!(media.attachments[0].kind, kind, "{mime}");
        assert_eq!(
            media.attachments[0].mime_type.as_deref(),
            normalized,
            "{mime}"
        );
    }
}

#[test]
fn stickers_are_images_unless_video_or_animated() {
    for (flags, kind) in [
        (json!({}), AttachmentKind::Image),
        (json!({"is_animated": true}), AttachmentKind::Other),
        (
            json!({"is_video": true, "is_animated": true}),
            AttachmentKind::Video,
        ),
    ] {
        let mut sticker = json!({"file_id": FILE_ID, "file_unique_id": "AgADstk", "width": 512, "height": 512, "emoji": "🙂"});
        sticker
            .as_object_mut()
            .unwrap()
            .extend(flags.as_object().unwrap().clone());
        let media = native(json!({ "sticker": sticker }));
        assert_eq!(media.attachments[0].kind, kind);
        assert_eq!(media.attachments[0].emoji.as_deref(), Some("🙂"));
        assert_eq!(media.nyxid_type, "image");
    }
}

#[test]
fn invalid_identifiers_degrade_to_an_item_reference_without_a_locator() {
    let media = native(json!({
        "video": {"file_id": FILE_ID, "file_unique_id": "bad id", "thumbnail": {"file_id": "AAQthumb", "file_unique_id": "AQADthumb"}},
        "voice": {"file_id": FILE_ID, "file_unique_id": "AgADvoice"},
    }));
    let video = &media.attachments[0];
    assert_eq!(video.reference, "telegram:item-0");
    assert!(video.source.is_none() && video.thumbnail.is_none());
    let voice = &media.attachments[1];
    assert_eq!(voice.reference, "telegram:AgADvoice");
    assert!(matches!(
        voice.source,
        Some(AttachmentSource::Telegram { .. })
    ));
}

#[test]
fn thumbnails_are_native_only_and_kept_only_with_valid_identifiers() {
    let video = |thumbnail: Value| json!({"video": {"file_id": FILE_ID, "file_unique_id": "AgADvid", "thumbnail": thumbnail}});
    let valid =
        json!({"file_id": "AAQthumb", "file_unique_id": "AQADthumb", "width": 320, "height": 180});
    assert!(native(video(valid.clone())).attachments[0]
        .thumbnail
        .is_some());
    assert!(native(video(
        json!({"file_id": "AAQ thumb", "file_unique_id": "AQADthumb"})
    ))
    .attachments[0]
        .thumbnail
        .is_none());
    assert!(relay(
        video(valid),
        json!([{"content_type": "video", "url": FILE_ID}])
    )
    .attachments[0]
        .thumbnail
        .is_none());
}

#[test]
fn unexpected_field_shapes_are_skipped_without_refusing() {
    let media = native(json!({"photo": "not sizes", "document": 7, "audio": {"file_id": FILE_ID}}));
    assert_eq!(media.attachments.len(), 1);
    assert_eq!(media.attachments[0].variant, Some(MediaVariant::Audio));
    assert!(native(json!({"photo": [], "video": null}))
        .attachments
        .is_empty());
}

#[test]
fn album_ids_are_kept_only_within_their_bounds() {
    let album = |id: Value| native(json!({"photo": [], "media_group_id": id})).album_id;
    assert_eq!(album(json!("13579")).as_deref(), Some("13579"));
    assert_eq!(album(json!("x".repeat(128))).map(|id| id.len()), Some(128));
    for invalid in [
        json!(""),
        json!("x".repeat(129)),
        json!("a\u{202E}b"),
        json!(13579),
    ] {
        assert_eq!(album(invalid.clone()), None, "{invalid}");
    }
}

/// Seeded xorshift64, so a failing case is reproducible from its number.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
    fn pick<T: Clone>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())].clone()
    }
}

fn string(rng: &mut Rng) -> Value {
    const PIECES: [&str; 16] = [
        "a", "Z", "9", "_", ".", " ", "文", "😀", "\u{0}", "\u{7}", "\u{85}", "\u{202E}",
        "\u{2066}", ";", "\n", "pdf",
    ];
    let length = rng.pick(&[0, 1, 5, 40, 128, 129, 255, 256, 300, 600]);
    let mut value = String::new();
    while value.len() < length {
        value.push_str(rng.pick(&PIECES));
    }
    Value::String(value)
}

fn number(rng: &mut Rng) -> Value {
    rng.pick(&[
        json!(0),
        json!(1),
        json!(2048),
        json!(2049),
        json!(86_401),
        json!(100_001),
        json!(8 * 1024 * 1024 + 1),
        json!(u32::MAX),
        json!(u64::from(u32::MAX) + 1),
        json!(u64::MAX),
        json!(-1),
        json!(1.5),
        json!("12"),
        Value::Null,
    ])
}

fn id(rng: &mut Rng, max: usize) -> Value {
    match rng.below(6) {
        0 => json!(""),
        1 => json!("has space"),
        2 => json!("x".repeat(max + 1)),
        3 => number(rng),
        _ => {
            let length = 1 + rng.below(max);
            let id: String = (0..length)
                .map(|_| rng.pick(&['a', 'Z', '0', '9', '_', '-']))
                .collect();
            json!(id)
        }
    }
}

fn media(rng: &mut Rng, shared: &Value) -> Value {
    let mut object = Map::new();
    for key in ["width", "height", "duration", "length", "file_size"] {
        if rng.coin() {
            object.insert(key.into(), number(rng));
        }
    }
    for key in ["file_name", "emoji", "mime_type"] {
        if rng.coin() {
            object.insert(key.into(), string(rng));
        }
    }
    if rng.coin() {
        let mime = rng.pick(&[
            "Text/Plain; charset=utf-8",
            "image/jpeg",
            "video/",
            "IMAGE/PNG",
        ]);
        object.insert("mime_type".into(), json!(mime));
    }
    for key in ["is_video", "is_animated"] {
        if rng.coin() {
            object.insert(key.into(), json!(rng.coin()));
        }
    }
    if rng.below(4) > 0 {
        let file_id = if rng.coin() {
            shared.clone()
        } else {
            id(rng, 256)
        };
        object.insert("file_id".into(), file_id);
    }
    if rng.below(4) > 0 {
        object.insert("file_unique_id".into(), id(rng, 128));
    }
    if rng.coin() {
        let thumbnail =
            json!({"file_id": id(rng, 256), "file_unique_id": id(rng, 128), "width": number(rng)});
        object.insert("thumbnail".into(), thumbnail);
    }
    Value::Object(object)
}

fn message(rng: &mut Rng) -> Value {
    let shared = id(rng, 256);
    let mut object = Map::new();
    for field in MEDIA_FIELDS {
        if rng.below(3) > 0 {
            continue;
        }
        let value = match rng.below(8) {
            0 => rng.pick(&[json!(7), json!("x"), json!([]), Value::Null]),
            _ if field == "photo" => {
                let sizes = rng.below(6);
                Value::Array((0..sizes).map(|_| media(rng, &shared)).collect())
            }
            _ => media(rng, &shared),
        };
        object.insert(field.into(), value);
    }
    if rng.coin() {
        let album = if rng.coin() { string(rng) } else { number(rng) };
        object.insert("media_group_id".into(), album);
    }
    Value::Object(object)
}

/// Envelope items whose opaque urls sometimes name the message's file ids.
fn envelope(rng: &mut Rng, message: &Value) -> RelayAttachments {
    let mut urls = vec![json!(""), json!("unrelated")];
    for field in MEDIA_FIELDS {
        let value = &message[field];
        let objects = value
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![value.clone()]);
        urls.extend(
            objects
                .iter()
                .filter_map(|object| object.get("file_id").cloned()),
        );
    }
    let count = rng.below(40);
    let items = (0..count)
        .map(|_| match rng.below(6) {
            0 => json!(7),
            _ => json!({"content_type": "image", "url": rng.pick(&urls)}),
        })
        .collect();
    relay_list(Value::Array(items))
}

fn activity(media: TelegramMedia, source: &SourceKind) -> Activity {
    Activity {
        thread: None,
        schema_version: 1,
        event_id: EventId::new("message-1".to_owned()).unwrap(),
        kind: ActivityKind::Message {
            text: String::new(),
            attachments: media.attachments,
            mentions_bot: false,
            reply_to: None,
            quoted_message: None,
            album_id: media.album_id,
        },
        actor: Actor {
            id: "7".into(),
            display_name: None,
            kind: ActorKind::Human,
        },
        conversation: ActivityConversation {
            id: "42".into(),
            kind: ConversationKind::Private,
            thread_id: None,
        },
        occurred_at: "2026-10-07T00:00:00Z".into(),
        source: source.clone(),
        routing: None,
    }
}

#[test]
fn every_extraction_validates_for_both_transports() {
    let relay_source = SourceKind::NyxidRelay {
        platform: Platform::Telegram,
        route_id: "route-1".into(),
    };
    let native_source = SourceKind::Telegram {
        platform: Platform::Telegram,
        bot_id: "77".into(),
    };
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut extracted, mut located) = (0, 0);
    for case in 0..2_000 {
        let message = message(&mut rng);
        let list = envelope(&mut rng, &message);
        for (transport, label, source) in [
            (Transport::Relay(&list), "nyxid_relay", &relay_source),
            (Transport::Native, "telegram", &native_source),
        ] {
            let Some(media) = extract(&message, transport, label) else {
                continue;
            };
            extracted += media.attachments.len();
            located += media
                .attachments
                .iter()
                .filter(|attachment| attachment.source.is_some())
                .count();
            if let Err(error) = activity(media, source).validate() {
                panic!("case {case}: {error}\n{message}");
            }
        }
    }
    // The generator must exercise extraction and both locator outcomes.
    assert!(extracted > 2_000, "only {extracted} attachments extracted");
    assert!(
        located > 200 && located < extracted,
        "{located} of {extracted} located"
    );
}
