use super::{
    tests::{envelope, source},
    webhook::{self, Content, Error},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::activity::{AttachmentKind, AttachmentSource, MediaVariant};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(crate) fn message(kind: &str, id: &str, bytes: &[u8]) -> Value {
    let mime = match kind {
        "image" => "image/jpeg",
        "audio" => "audio/ogg; codecs=opus",
        "video" => "video/mp4",
        "sticker" => "image/webp",
        _ => "text/plain",
    };
    json!({"from":"447700900123","id":id,"timestamp":"1800000000","type":kind,
        (kind):{"id":"45678","mime_type":mime,"sha256":STANDARD.encode(Sha256::digest(bytes)),
            "url":"https://untrusted.example/do-not-fetch"}})
}

#[test]
fn signed_native_media_preserves_caption_voice_filename_and_source_without_temporary_url() {
    for (kind, expected, variant) in [
        ("image", AttachmentKind::Image, MediaVariant::Image),
        ("document", AttachmentKind::File, MediaVariant::Document),
        ("audio", AttachmentKind::Audio, MediaVariant::Voice),
        ("video", AttachmentKind::Video, MediaVariant::Video),
        ("sticker", AttachmentKind::Image, MediaVariant::Sticker),
    ] {
        let mut raw = message(kind, "wamid.media", b"media fixture");
        if kind == "audio" {
            raw[kind]["voice"] = true.into();
        }
        if kind == "sticker" {
            raw[kind]["animated"] = true.into();
        }
        if matches!(kind, "image" | "document" | "video") {
            raw[kind]["caption"] = "Review this".into();
        }
        if kind == "document" {
            raw[kind]["filename"] = "Report\u{202e}.txt".into();
        }
        let received = webhook::extract(&source(), &envelope(vec![raw]))
            .unwrap()
            .remove(0);
        let Content::Media {
            caption,
            attachment,
        } = received.content
        else {
            panic!()
        };
        assert_eq!(
            (attachment.kind, attachment.variant),
            (expected, Some(variant))
        );
        assert_eq!(
            caption,
            if matches!(kind, "image" | "document" | "video") {
                "Review this"
            } else {
                ""
            }
        );
        if kind == "audio" {
            assert_eq!(attachment.mime_type.as_deref(), Some("audio/ogg"));
        }
        if kind == "document" {
            assert_eq!(attachment.filename.as_deref(), Some("Report.txt"));
        }
        assert_eq!(
            (
                attachment.size_bytes,
                attachment.width,
                attachment.height,
                attachment.duration_seconds
            ),
            (None, None, None, None)
        );
        assert!(
            matches!(&attachment.source,Some(AttachmentSource::Whatsapp {message_id,media_id,sha256})
            if message_id=="wamid.media" && media_id=="45678" && sha256==&webhook::hex(&Sha256::digest(b"media fixture")))
        );
        assert!(!serde_json::to_string(&attachment)
            .unwrap()
            .contains("untrusted"));
    }
}

#[test]
fn malformed_media_and_changed_signed_facts_are_not_silently_dropped() {
    for (field, bad) in [
        ("id", json!("45678/path")),
        ("sha256", json!("bad")),
        ("voice", json!("yes")),
        ("mime_type", json!(42)),
    ] {
        let mut raw = message("audio", "wamid.media", b"data");
        raw["audio"][field] = bad;
        assert_eq!(
            webhook::extract(&source(), &envelope(vec![raw])),
            Err(Error::Message)
        );
    }
    for (field, bad) in [
        ("id", json!("99999")),
        ("caption", json!("changed")),
        ("sha256", json!(STANDARD.encode([1; 32]))),
    ] {
        let raw = message("image", "wamid.same", b"data");
        let mut changed = raw.clone();
        changed["image"][field] = bad;
        assert_eq!(
            webhook::extract(&source(), &envelope(vec![raw, changed])),
            Err(Error::Message)
        );
    }
    let mut raw = message("sticker", "wamid.media", b"data");
    raw["sticker"]["animated"] = "true".into();
    assert_eq!(
        webhook::extract(&source(), &envelope(vec![raw])),
        Err(Error::Message)
    );
}

#[test]
fn media_digest_accepts_meta_base64_or_hex_and_rejects_other_shapes() {
    use cmaeg_core::channel::whatsapp::media_sha256;
    let bytes = [17; 32];
    assert_eq!(media_sha256(&STANDARD.encode(bytes)), Some(bytes));
    assert_eq!(media_sha256(&"11".repeat(32)), Some(bytes));
    for value in [
        "11".repeat(31),
        "11".repeat(33),
        STANDARD.encode([17; 31]),
        STANDARD.encode([17; 33]),
        "z".repeat(64),
    ] {
        assert_eq!(media_sha256(&value), None);
    }
}
