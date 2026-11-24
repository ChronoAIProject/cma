use crate::channels::{routing_fixture, write_gate};
use serde_json::json;

#[test]
fn telegram_member_join_source_is_optional_in_legacy_records_and_removal_survives_shutdown() {
    let mut input = routing_fixture::routed();
    input["sources"] = json!([routing_fixture::telegram()]);
    input["sources"][0]["message_events"] = true.into();
    input["sources"][0]["admission"] = json!({
        "type": "scoped", "senders": {"type": "open"},
        "chats": {"type": "allowlist", "ids": ["-100", "-101"]}, "groups": "all",
    });
    let legacy = routing_fixture::definition(input.clone());
    assert!(legacy.to_blob_json()["sources"][0]
        .get("member_join_chat_ids")
        .is_none());
    input["sources"][0]["member_join_chat_ids"] = json!(["-100", "-101"]);
    let enabled = routing_fixture::definition(input.clone());
    enabled.validate().unwrap();

    let disabled = write_gate::WriteFlags::default();
    assert_eq!(
        write_gate::check(Some(&legacy), &enabled, disabled)
            .unwrap_err()
            .pointer,
        "/sources/0/member_join_chat_ids"
    );
    write_gate::check(
        Some(&legacy),
        &enabled,
        write_gate::WriteFlags {
            telegram_member_joins: true,
            ..disabled
        },
    )
    .unwrap();
    write_gate::check(Some(&enabled), &enabled, disabled).unwrap();
    input["sources"][0]["member_join_chat_ids"] = json!(["-100"]);
    let reduced = routing_fixture::definition(input.clone());
    write_gate::check(Some(&enabled), &reduced, disabled).unwrap();
    write_gate::check(Some(&enabled), &legacy, disabled).unwrap();
    assert!(write_gate::check(Some(&reduced), &enabled, disabled).is_err());

    let mut replacement = enabled.clone();
    replacement.secrets.telegram_webhook_secret = Some(cmaeg_core::channel::Secret::new("rotated"));
    assert!(write_gate::check(Some(&enabled), &replacement, disabled).is_err());

    for ids in [json!(["-100", "-100"]), json!(["-200"]), json!(["-0100"])] {
        input["sources"][0]["member_join_chat_ids"] = ids;
        let invalid = routing_fixture::definition(input.clone());
        assert_eq!(
            invalid.validate().unwrap_err().pointer,
            "/sources/0/member_join_chat_ids"
        );
    }
}
