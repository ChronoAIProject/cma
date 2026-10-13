//! The channel's default reply destination must name something attached.
use cmaeg_core::{channel::DefaultReply, ValidationError};
use serde_json::{json, Value};

use super::fixture::{definition, relay, routed, ROUTE};

fn with_reply(reply: Value) -> Value {
    let mut value = routed();
    value["routing"]["default_reply"] = reply;
    value
}

fn expect(value: Value, pointer: &str, message: &str) {
    assert_eq!(
        definition(value).validate(),
        Err(ValidationError::new(pointer, message))
    );
}

const SOURCE: &str = "must name an attached source of this type";

#[test]
fn every_variant_is_accepted_when_its_destination_exists() {
    for reply in [
        json!({"type": "telegram_chat", "source_id": "telegram", "chat_id": "-1001234", "thread_id": "7"}),
        json!({"type": "telegram_chat", "source_id": "telegram", "chat_id": "42"}),
        json!({"type": "relay_conversation", "source_id": "relay_lark", "route_id": ROUTE}),
        json!({"type": "relay_conversation", "source_id": "relay_lark", "route_id": ROUTE.to_uppercase()}),
        json!({"type": "sinks"}),
    ] {
        let mut value = with_reply(reply.clone());
        if reply["route_id"] == ROUTE.to_uppercase() {
            value["sources"][1]["route_ids"] = json!([ROUTE.to_uppercase()]);
        }
        let definition = definition(value);
        definition.validate().unwrap();
        let stored = definition.routing.unwrap().default_reply.unwrap();
        assert_eq!(serde_json::to_value(&stored).unwrap(), reply);
    }
}

#[test]
fn a_telegram_chat_needs_the_native_source_and_integer_ids() {
    expect(
        with_reply(json!({"type": "telegram_chat", "source_id": "relay_lark", "chat_id": "1"})),
        "/routing/default_reply/source_id",
        SOURCE,
    );
    let mut detached =
        with_reply(json!({"type": "telegram_chat", "source_id": "telegram", "chat_id": "1"}));
    detached["sources"] = json!([relay("lark", ROUTE)]);
    detached["partition"]["key"] = "conversation".into();
    expect(detached, "/routing/default_reply/source_id", SOURCE);
    for chat in ["0", "abc", "+5", "01", "", "99999999999999999999"] {
        expect(
            with_reply(json!({"type": "telegram_chat", "source_id": "telegram", "chat_id": chat})),
            "/routing/default_reply/chat_id",
            "must be a non-zero integer",
        );
    }
    for thread in ["0", "-1", "x", "007"] {
        expect(
            with_reply(
                json!({"type": "telegram_chat", "source_id": "telegram", "chat_id": "1",
                "thread_id": thread}),
            ),
            "/routing/default_reply/thread_id",
            "must be a positive integer",
        );
    }
}

#[test]
fn a_relay_conversation_needs_an_attached_relay_and_one_of_its_uuid_routes() {
    for source in ["relay_slack", "telegram", "custom"] {
        expect(
            with_reply(
                json!({"type": "relay_conversation", "source_id": source, "route_id": ROUTE}),
            ),
            "/routing/default_reply/source_id",
            SOURCE,
        );
    }
    for route in [
        "route-1",
        "5b3f0c6e1c1d4d399a8e2f5b6c7d8e9f",
        "{5b3f0c6e-1c1d-4d39-9a8e-2f5b6c7d8e9f}",
    ] {
        expect(
            with_reply(
                json!({"type": "relay_conversation", "source_id": "relay_lark", "route_id": route}),
            ),
            "/routing/default_reply/route_id",
            "must be a UUID",
        );
    }
    expect(
        with_reply(
            json!({"type": "relay_conversation", "source_id": "relay_lark",
            "route_id": "00000000-0000-4000-8000-000000000000"}),
        ),
        "/routing/default_reply/route_id",
        "must be a route of that relay source",
    );
}

#[test]
fn sinks_need_a_result_webhook() {
    let mut value = with_reply(json!({"type": "sinks"}));
    value["reply"]["sinks"] = json!([{"type": "source"}, {"type": "sync"}]);
    expect(
        value,
        "/routing/default_reply",
        "requires a result_webhook sink",
    );
}

#[test]
fn the_variants_keep_their_wire_names() {
    let sinks: DefaultReply = serde_json::from_value(json!({"type": "sinks"})).unwrap();
    assert_eq!(sinks, DefaultReply::Sinks {});
    assert!(serde_json::from_value::<DefaultReply>(json!({"type": "sinks", "url": "x"})).is_err());
}
