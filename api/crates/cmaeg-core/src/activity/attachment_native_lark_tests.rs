use super::*;
use crate::activity::Activity;
use serde_json::json;

fn activity() -> serde_json::Value {
    json!({"schema_version":1,"event_id":"native-media","kind":{"type":"message","text":"","mentions_bot":false,"attachments":[{
        "reference":"native-lark:om_original:file_original","kind":"video",
        "source":{"type":"native_lark","message_id":"om_original","resource_key":"file_original","resource_kind":"file","source_revision":"a".repeat(64)},
        "thumbnail":{"type":"native_lark","message_id":"om_original","resource_key":"img_cover","resource_kind":"image","source_revision":"a".repeat(64)}
    }]},"actor":{"id":"ou_sender","kind":"human"},"conversation":{"id":"oc_chat","kind":"private"},"occurred_at":"2026-10-28T00:00:00Z",
    "source":{"type":"native_lark","region":"feishu","app_id":"cli_app","tenant_key":"tenant","bot_open_id":"ou_bot"}})
}

#[test]
fn native_locator_and_image_cover_roundtrip_while_wrong_sources_and_unsafe_keys_fail() {
    let value = activity();
    let parsed: Activity = serde_json::from_value(value.clone()).unwrap();
    parsed.validate().unwrap();
    let locator: AttachmentSource =
        serde_json::from_value(value["kind"]["attachments"][0]["source"].clone()).unwrap();
    assert_eq!(format!("{locator:?}"), "NativeLark([REDACTED])");
    for (pointer, changed) in [
        ("/kind/attachments/0/source/message_id", json!("oc_wrong")),
        (
            "/kind/attachments/0/source/resource_key",
            json!("../secret"),
        ),
        (
            "/kind/attachments/0/source/source_revision",
            json!("a".repeat(63)),
        ),
        (
            "/kind/attachments/0/source/source_revision",
            json!("A".repeat(64)),
        ),
        ("/kind/attachments/0/thumbnail/resource_kind", json!("file")),
        ("/source", json!({"type":"custom"})),
        (
            "/source",
            json!({"type":"telegram","platform":"telegram","bot_id":"77"}),
        ),
    ] {
        let mut changed_value = value.clone();
        *changed_value.pointer_mut(pointer).unwrap() = changed;
        assert!(
            serde_json::from_value::<Activity>(changed_value)
                .unwrap()
                .validate()
                .is_err(),
            "{pointer}"
        );
    }
    let mut bare = value;
    bare["kind"]["attachments"][0]["source"]["resource_key"] =
        "00000000-1111-2222-3333-44444444444g".into();
    serde_json::from_value::<Activity>(bare)
        .unwrap()
        .validate()
        .unwrap();
}
