use super::{
    tests::{envelope, source},
    webhook::{self, Content, Error},
};
use serde_json::{json, Value};
pub(crate) fn edit(id: &str, target: &str, message: Value) -> Value {
    json!({"from":"447700900123","id":id,"timestamp":"1800000000","type":"edit",
        "edit":{"original_message_id":target,"message":message}})
}
pub(crate) fn revoke(id: &str, target: &str) -> Value {
    json!({"from":"447700900123","id":id,"timestamp":"1800000000","type":"revoke",
        "revoke":{"original_message_id":target}})
}
pub(crate) fn edited_text(id: &str, target: &str, text: &str) -> Value {
    edit(id, target, json!({"type":"text","text":{"body":text}}))
}
#[test]
fn native_whatsapp_changes_parse_documented_shapes_and_ignore_media_url() {
    let mut media = super::media_tests::message("image", "wamid.original", b"same media");
    media["image"]["caption"] = "Updated image caption".into();
    let messages = webhook::extract(
        &source(),
        &envelope(vec![
            edited_text("wamid.edit", "wamid.original", "updated"),
            edit("wamid.caption", "wamid.original", media),
            revoke("wamid.revoke", "wamid.original"),
        ]),
    )
    .unwrap();
    assert!(
        matches!(&messages[0].content, Content::Edit { target, text, media: None } if target=="wamid.original" && text=="updated")
    );
    assert!(
        matches!(&messages[1].content, Content::Edit { media: Some(_), text, .. } if text=="Updated image caption")
    );
    assert!(matches!(&messages[2].content, Content::Revoke { target } if target=="wamid.original"));
    assert!(!serde_json::to_string(&messages)
        .unwrap()
        .contains("untrusted.example"));
}
#[test]
fn native_whatsapp_changes_refuse_groups_ambiguous_and_malformed_payloads() {
    let base = edited_text("wamid.edit", "wamid.original", "updated");
    for (pointer, mutation) in [
        ("/group_id", json!("group_A")),
        ("/recipient_type", json!("group")),
        ("/edit/original_message_id", json!("wamid.edit")),
        ("/edit/original_message_id", Value::Null),
        ("/edit/message/type", json!("audio")),
        ("/edit/message/text/body", json!(42)),
        ("/edit/message/text/body", json!("")),
        ("/edit/message/image", json!({})),
        ("/edit/message/group", json!({})),
        ("/revoke", json!({"original_message_id":"wamid.other"})),
    ] {
        let mut value = base.clone();
        let (parent, field) = pointer.rsplit_once('/').unwrap();
        value.pointer_mut(parent).unwrap()[field] = mutation;
        assert_eq!(
            webhook::extract(&source(), &envelope(vec![value])),
            Err(Error::Message),
            "{pointer}"
        );
    }
}
#[test]
fn native_whatsapp_changes_gate_requires_new_consent_but_allows_retention_and_removal() {
    use crate::channels::{
        definition::fixture,
        write_gate::{self, WriteFlags},
    };
    let mut legacy = fixture::with_agent_key("whatsapp-events-fixture");
    legacy.sources = vec![source()];
    let mut enabled = legacy.clone();
    let cmaeg_core::channel::Source::Whatsapp { message_events, .. } = &mut enabled.sources[0]
    else {
        panic!()
    };
    *message_events = true;
    let flags = WriteFlags {
        native_whatsapp: true,
        ..Default::default()
    };
    assert!(write_gate::check(Some(&legacy), &enabled, flags).is_err());
    assert!(write_gate::check(Some(&enabled), &enabled, flags).is_ok());
    assert!(write_gate::check(Some(&enabled), &legacy, flags).is_ok());
    assert!(write_gate::check(
        Some(&legacy),
        &enabled,
        WriteFlags {
            native_whatsapp_message_events: true,
            ..flags
        }
    )
    .is_ok());
}
