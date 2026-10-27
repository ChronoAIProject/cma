use super::content::*;
use cmaeg_core::{
    activity::{ActivityKind, SourceKind},
    channel::{LarkResourceKind, Source},
};
use serde_json::json;

#[test]
fn all_documented_standalone_resource_shapes_preserve_exact_key_and_declared_facts() {
    for (message_type, content, kind) in [
        (
            "image",
            json!({"image_key":"img_original"}),
            AttachmentKind::Image,
        ),
        (
            "file",
            json!({"file_key":"00000000-1111-2222-3333-44444444444g","file_name":"report.json"}),
            AttachmentKind::File,
        ),
        (
            "audio",
            json!({"file_key":"file_audio","duration":2001}),
            AttachmentKind::Audio,
        ),
        (
            "media",
            json!({"file_key":"file_video","image_key":"img_cover","file_name":"video.mp4","duration":2000}),
            AttachmentKind::Video,
        ),
    ] {
        let (text, resources) = parse(message_type, &content).unwrap().unwrap();
        assert!(text.is_empty());
        let items = attachments(&resources, "om_original", &"a".repeat(64));
        assert_eq!(items[0].kind, kind);
        assert!(items[0].mime_type.is_none());
        assert!(items[0].size_bytes.is_none());
        if kind == AttachmentKind::Audio {
            assert_eq!(items[0].duration_seconds, Some(3));
        }
        if kind == AttachmentKind::Video {
            assert!(
                matches!(&items[0].thumbnail, Some(AttachmentSource::NativeLark { resource_key, resource_kind: LarkResourceKind::Image, .. }) if resource_key == "img_cover")
            );
        }
        let mut activity = crate::ingress::routing_plan::fixture::message("");
        activity.source = SourceKind::NativeLark(super::test_support::identity());
        let ActivityKind::Message { attachments, .. } = &mut activity.kind else {
            panic!()
        };
        *attachments = items;
        activity.validate().unwrap();
        activity.source = SourceKind::Custom;
        assert!(
            activity.validate().is_err(),
            "native resource must never become custom authority"
        );
    }
}

#[test]
fn flat_posts_keep_text_mentions_images_videos_and_covers_without_scanning_untrusted_shapes() {
    let post = json!({"title":"Title","content":[
        [{"tag":"text","text":"hello "},{"tag":"at","user_id":"@_user_1","user_name":""},{"tag":"a","text":"site","href":"https://example.org"}],
        [{"tag":"img","image_key":"img_one"}],
        [{"tag":"media","file_key":"file_two","image_key":"img_cover"}],
        [{"tag":"emotion","emoji_type":"SMILE"},{"tag":"hr"},{"tag":"code_block","language":"RUST","text":"let x = 1;"}]
    ], "content_v2":[[{"tag":"md","text":"![unauthorized](img_hidden)"}]]});
    let (text, resources) = parse("post", &post).unwrap().unwrap();
    assert!(text.contains("Title\nhello @_user_1site (https://example.org)"));
    assert!(text.contains(":SMILE:"));
    assert!(text.contains("let x = 1;"));
    assert_eq!(resources.len(), 2);
    assert!(!serde_json::to_string(&resources)
        .unwrap()
        .contains("img_hidden"));
    for tag in ["file", "audio", "md", "button", "unknown"] {
        let post =
            json!({"title":"","content":[[{"tag":tag,"file_key":"file_hidden","text":"hidden"}]]});
        assert!(parse("post", &post).is_err(), "unproved tag {tag}");
    }
    assert!(parse(
        "post",
        &json!({"en_us":{"title":"","content":[[{"tag":"img","image_key":"img_hidden"}]]}})
    )
    .is_err());
    for kind in ["sticker", "folder", "interactive", "merge_forward", "video"] {
        assert!(parse(kind, &json!({"file_key":"file_hidden"}))
            .unwrap()
            .is_none());
    }
}

#[test]
fn resource_key_count_metadata_and_text_bounds_refuse_the_entire_message() {
    for key in [
        "",
        "../file",
        "file?type=image",
        "file%2fother",
        "file secret",
        "https://example.org/file",
    ] {
        assert!(parse("file", &json!({"file_key":key})).is_err(), "{key}");
    }
    for content in [
        json!({"file_key":"file_ok","file_name":"\nsecret"}),
        json!({"file_key":"file_ok","duration":-1}),
        json!({"file_key":"file_ok","duration":86400001}),
        json!({"file_key":"x".repeat(1025)}),
    ] {
        assert!(parse("audio", &content).is_err());
    }
    assert!(parse(
        "post",
        &json!({"title":"","content":[vec![json!({"tag":"img","image_key":"img_same"});17]]})
    )
    .is_err());
    assert!(parse(
        "post",
        &json!({"title":"","content":[vec![json!({"tag":"text","text":"x"});1025]]})
    )
    .is_err());
    assert!(parse("text", &json!({"text":"x".repeat(32769)})).is_err());
}

#[test]
fn media_messages_use_original_stable_identity_and_legacy_text_encoding_is_unchanged() {
    let Source::NativeLark(source) = super::test_support::source() else {
        panic!()
    };
    let raw = super::test_support::text("om_original", "hello", 1_800_000_000);
    let message = super::message::extract(&source, &raw, 1_800_000_000)
        .unwrap()
        .unwrap();
    let encoded = serde_json::to_value(&message).unwrap();
    assert!(encoded.get("resources").is_none());
    assert_eq!(
        parse("text", &json!({"text":"   "})).unwrap().unwrap().0,
        "   "
    );
    assert_eq!(
        serde_json::from_value::<super::message::Message>(encoded).unwrap(),
        message
    );
    let mut raw = raw;
    raw["event"]["message"]["message_type"] = "image".into();
    raw["event"]["message"]["content"] = json!({"image_key":"img_one"}).to_string().into();
    let media = super::message::extract(&source, &raw, 1_800_000_000)
        .unwrap()
        .unwrap();
    assert!(media.has_resources());
    assert_eq!(
        media.event_id(&source.identity),
        message.event_id(&source.identity)
    );
    assert_ne!(media, message);
}
