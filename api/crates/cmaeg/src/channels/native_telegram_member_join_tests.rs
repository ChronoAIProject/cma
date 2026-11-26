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

#[test]
fn telegram_member_join_public_update_distinguishes_omitted_and_empty_lists() {
    use crate::channels::{definition_sources::build_sources, SourceRequest};

    let mut input = routing_fixture::routed();
    let mut source = routing_fixture::telegram();
    source["message_events"] = true.into();
    source["admission"] = json!({
        "type": "scoped", "senders": {"type": "open"},
        "chats": {"type": "allowlist", "ids": ["-100"]}, "groups": "all",
    });
    input["sources"] = json!([source]);
    input["sources"][0]["member_join_chat_ids"] = json!(["-100"]);
    let saved = routing_fixture::definition(input);
    saved.validate().unwrap();
    let project = |body| {
        let request: SourceRequest = serde_json::from_value(body).unwrap();
        build_sources(&[request], Some(&saved)).unwrap()
    };
    assert_eq!(project(source.clone()), saved.sources);
    source["member_join_chat_ids"] = json!([]);
    assert!(matches!(&project(source.clone())[0],
        cmaeg_core::channel::Source::Telegram { member_join_chat_ids, .. } if member_join_chat_ids.is_empty()));
    source["message_events"] = false.into();
    for ids in [None, Some(json!(["-100"]))] {
        let mut source = source.clone();
        if let Some(ids) = ids {
            source["member_join_chat_ids"] = ids;
        } else {
            source
                .as_object_mut()
                .unwrap()
                .remove("member_join_chat_ids");
        }
        let mut next = saved.clone();
        next.sources = project(source);
        next.validate().unwrap();
        write_gate::check(Some(&saved), &next, write_gate::WriteFlags::default()).unwrap();
        assert!(matches!(&next.sources[0],
            cmaeg_core::channel::Source::Telegram { member_join_chat_ids, .. } if member_join_chat_ids.is_empty()));
    }
}
