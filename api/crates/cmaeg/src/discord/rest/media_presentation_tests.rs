//! Generated previews retain semantics while documented access locators may refresh.
use super::media_edit::{observe, payload, snapshot};
use cmaeg_core::message::NativeMediaKind;
use serde_json::{json, Value};

fn message() -> Value {
    json!({"id":"800","channel_id":"99","author":{"id":"77","bot":true},"type":0,"content":"https://example.com","flags":4096,
        "embeds":[{"type":"link","title":"Example","url":"https://example.com/?ex=source","thumbnail":{"url":"https://cdn.discordapp.com/attachments/99/900/image.png?ex=1&is=2&hm=3&width=20","proxy_url":"https://proxy.example/old","width":20,"height":30}}],
        "attachments":[{"id":"900","filename":"SPOILER_report.bin","size":4,"content_type":"application/octet-stream","flags":8,"description":"alt","title":"report","placeholder":"abc","placeholder_version":1}]})
}
fn read(body: &Value) -> crate::native_media::model::Snapshot {
    snapshot(body, "77", "99", "800", NativeMediaKind::File).unwrap()
}
#[test]
fn cdn_signature_and_proxy_refresh_do_not_hide_semantic_preview_changes() {
    let mut body = message();
    let before = read(&body);
    body["embeds"][0]["thumbnail"]["url"] =
        "https://cdn.discordapp.com/attachments/99/900/image.png?ex=4&is=5&hm=6&width=20".into();
    body["embeds"][0]["thumbnail"]["proxy_url"] = "https://proxy.example/new".into();
    body["attachments"][0]["url"] = "https://cdn.discordapp.com/refreshed".into();
    assert_eq!(before, read(&body));
    for (pointer, changed) in [
        ("/embeds/0/url", json!("https://example.com/?ex=changed")),
        (
            "/embeds/0/thumbnail/url",
            json!("https://cdn.discordapp.com/attachments/99/900/image.png?ex=4&width=40"),
        ),
        ("/embeds/0/title", json!("Changed")),
        ("/attachments/0/title", json!("Changed")),
        ("/attachments/0/flags", json!(40)),
        ("/type", json!(19)),
    ] {
        let mut changed_body = body.clone();
        *changed_body.pointer_mut(pointer).unwrap() = changed;
        assert_ne!(before, read(&changed_body), "{pointer}");
    }
}
#[test]
fn generated_preview_limits_retire_authority_but_structural_modes_are_ambiguous() {
    for embeds in [
        json!([{"type":"link","title":"x".repeat(257)}]),
        json!([{"type":"link","new_optional_metadata":true}]),
        Value::Array(
            (0..11)
                .map(|_| json!({"type":"link","url":"https://example.com"}))
                .collect(),
        ),
    ] {
        let mut body = message();
        body["embeds"] = embeds;
        assert!(
            !observe(&body, "77", "99", "800", NativeMediaKind::File)
                .unwrap()
                .eligible
        );
        assert!(snapshot(&body, "77", "99", "800", NativeMediaKind::File).is_err());
    }
    for embeds in [
        json!([{"type":"rich"}]),
        json!([{"type":"poll_result"}]),
        json!([{"type":"link","components":[]}]),
        json!([{"type":"link","title":"x".repeat(257),"description":42}]),
    ] {
        let mut body = message();
        body["embeds"] = embeds;
        assert!(observe(&body, "77", "99", "800", NativeMediaKind::File).is_err());
    }
    let mut body = message();
    body["poll"] = json!({"question":{"text":"Question"}});
    assert!(observe(&body, "77", "99", "800", NativeMediaKind::File).is_err());
}
#[test]
fn retained_spoiler_payload_omits_server_embeds_flags_and_filename() {
    let before = read(&message());
    let patch = serde_json::from_value(json!({"caption":"new"})).unwrap();
    assert_eq!(
        payload(&patch, &before, None).unwrap(),
        json!({"attachments":[{"id":"900","description":"new","is_spoiler":true}]})
    );
    let patch = serde_json::from_value(json!({"text":"new"})).unwrap();
    assert_eq!(
        payload(&patch, &before, None).unwrap(),
        json!({"content":"new","allowed_mentions":{"parse":[],"replied_user":false}})
    );
    let file = crate::media::outbound::PreparedFile {
        bytes: vec![1, 2, 3, 4],
        filename: "new.bin".into(),
        mime: "application/octet-stream".into(),
        caption: None,
    };
    assert_eq!(
        payload(&patch, &before, Some(&file)).unwrap()["attachments"],
        json!([{"id":0,"filename":"new.bin","description":"alt","is_spoiler":true}])
    );
    let mut body = message();
    body["attachments"][0]["flags"] = 0.into();
    assert!(observe(&body, "77", "99", "800", NativeMediaKind::File).is_err());
}
