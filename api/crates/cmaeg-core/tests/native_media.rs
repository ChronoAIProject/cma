use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::message::{OutboundMessage, MAX_NATIVE_INLINE_BYTES};
use serde_json::{json, Value};

fn message(source: Value) -> OutboundMessage {
    serde_json::from_value(json!({"native_attachments":[{"kind":"file","source":source}]})).unwrap()
}

#[test]
fn native_media_is_explicit_and_legacy_messages_keep_their_serialized_shape() {
    let legacy = json!({"text":"hello","blocks":[]});
    let legacy_message: OutboundMessage = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(legacy_message).unwrap(), legacy);
    message(json!({"type":"url","url":"https://media.example/report.pdf"}))
        .validate()
        .unwrap();
}

#[test]
fn inline_limit_counts_decoded_bytes_across_all_attachments() {
    let at_limit = STANDARD.encode(vec![3; MAX_NATIVE_INLINE_BYTES]);
    let valid = message(json!({"type":"base64","data":at_limit}));
    valid.validate().unwrap();
    let mut cumulative = valid.clone();
    cumulative
        .native_attachments
        .extend(message(json!({"type":"base64","data":"AQ=="})).native_attachments);
    assert!(cumulative.validate().is_err());
    for data in [
        STANDARD.encode(vec![3; MAX_NATIVE_INLINE_BYTES + 1]),
        STANDARD.encode(vec![3; MAX_NATIVE_INLINE_BYTES + 2]),
        "".into(),
        "AQ".into(),
        "%%%=".into(),
        "AR==".into(),
    ] {
        assert!(message(json!({"type":"base64","data":data}))
            .validate()
            .is_err());
    }
}

#[test]
fn media_metadata_and_url_shape_are_bounded_before_transport() {
    for url in [
        "http://media.example/a",
        "file:///etc/passwd",
        "https://user:pass@media.example/a",
        "https://media.example/a#secret",
        "https://media.example/a b",
    ] {
        assert!(message(json!({"type":"url","url":url})).validate().is_err());
    }
    for patch in [
        json!({"filename":"../report.pdf"}),
        json!({"filename":"folder\\report.pdf"}),
        json!({"filename":".."}),
        json!({"filename":"x".repeat(256)}),
        json!({"mime_type":"text/plain\r\nx-header: value"}),
        json!({"mime_type":"text/plain; charset=utf-8"}),
        json!({"caption":"x".repeat(1025)}),
        json!({"caption":"hidden\u{202e}text"}),
    ] {
        let mut body = json!({"native_attachments":[{"kind":"file","source":{"type":"base64","data":"AQ=="}}]});
        body["native_attachments"][0]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let request: OutboundMessage = serde_json::from_value(body).unwrap();
        assert!(request.validate().is_err());
    }
    let mut too_many = message(json!({"type":"base64","data":"AQ=="}));
    too_many.native_attachments = vec![too_many.native_attachments[0].clone(); 5];
    assert!(too_many.validate().is_err());
    let unsupported = json!({"native_attachments":[{"kind":"other","source":{"type":"url","url":"https://media.example/a"}}]});
    assert!(serde_json::from_value::<OutboundMessage>(unsupported).is_err());
}

#[test]
fn debug_does_not_expose_media_bytes_urls_or_metadata() {
    let value =
        message(json!({"type":"url","url":"https://media.example/private.png?signature=hidden"}));
    let debug = format!("{:?}", value.native_attachments);
    assert!(!debug.contains("media.example"));
    assert!(!debug.contains("signature"));
    assert!(!format!(
        "{:?}",
        message(json!({"type":"base64","data":"c2VjcmV0"})).native_attachments
    )
    .contains("c2VjcmV0"));
}

#[test]
fn voice_and_animation_are_explicit_additive_kinds_without_changing_audio_or_bytes() {
    for kind in ["audio", "video", "voice", "animation", "sticker"] {
        let wire = json!({"blocks":[],"native_attachments":[{"kind":kind,"source":{"type":"base64","data":"AQIDBA=="}}]});
        let message: OutboundMessage = serde_json::from_value(wire.clone()).unwrap();
        message.validate().unwrap();
        assert_eq!(serde_json::to_value(&message).unwrap(), wire);
    }
    for kind in ["voice", "animation", "sticker"] {
        let patch: cmaeg_core::message::NativeMediaUpdate =
            serde_json::from_value(json!({"replacement":{
                "kind":kind,"source":{"type":"base64","data":"AQIDBA=="}
            }}))
            .unwrap();
        assert!(
            patch.validate().is_err(),
            "presentation must not gain media edit authority"
        );
    }
}
