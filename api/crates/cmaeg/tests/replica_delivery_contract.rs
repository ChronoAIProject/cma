#[path = "support/replica_delivery.rs"]
mod replica_delivery;

use cmaeg::turns::routed::TargetState;
use replica_delivery::{observe, routed_channel, OutboundEvidence};
use serde_json::{json, Value};

fn completed(status: Value) -> Value {
    json!({
        "kind":"ordinary_route_execution_v1", "id":"target", "root":"root",
        "owner":"fixture-owner", "request_hash":"request", "retained_until":200,
        "event_expires_at":100,
        "phase":{"state":"completed", "response_id":"response", "durable_cursor":1},
        "ownership":{"worker":"worker", "claim":"claim", "epoch":1, "until":100},
        "delivery":{"response_id":"response", "cursor":1, "frame_hash":"frame",
            "effect_hash":"effect", "terminal":true, "status":status}
    })
}
fn receipt(outcome: Value) -> Value {
    json!({"message_ref":null,"rendering":{"outcome":"rendered","blocks":[]},"delivery":outcome})
}
fn observation(value: Value) -> replica_delivery::Observation {
    observe(&serde_json::from_value::<TargetState>(value).unwrap())
}

#[test]
fn terminal_provider_state_cannot_hide_unresolved_or_failed_delivery() {
    let cases = [
        (json!({"state":"intent"}), OutboundEvidence::Intent),
        (json!({"state":"unknown"}), OutboundEvidence::Unknown),
        (
            json!({"state":"settled","receipt":null}),
            OutboundEvidence::NoEffect,
        ),
        (
            json!({"state":"partial","receipt":receipt(json!({"outcome":"unknown","reason":"lost reply"}))}),
            OutboundEvidence::Partial,
        ),
        (
            json!({"state":"settled","receipt":receipt(json!({"outcome":"unknown","reason":"lost reply"}))}),
            OutboundEvidence::Unknown,
        ),
        (
            json!({"state":"settled","receipt":receipt(json!({"outcome":"rejected","reason":"refused"}))}),
            OutboundEvidence::Rejected,
        ),
        (
            json!({"state":"settled","receipt":receipt(json!({"outcome":"rate_limited","retry_after":1}))}),
            OutboundEvidence::RateLimited,
        ),
        (
            json!({"state":"settled","receipt":receipt(json!({"outcome":"unsupported"}))}),
            OutboundEvidence::Unsupported,
        ),
    ];
    for (status, expected) in cases {
        let observed = observation(completed(status));
        assert_eq!(observed.outbound, expected);
        assert!(!observed.confirmed_terminal_text());
    }
    let mut absent = completed(json!({"state":"unknown"}));
    absent.as_object_mut().unwrap().remove("delivery");
    assert_eq!(observation(absent).outbound, OutboundEvidence::Missing);
}

#[test]
fn confirmed_terminal_delivery_requires_matching_committed_response_and_cursor() {
    let value = completed(json!({"state":"settled", "receipt":receipt(
        json!({"outcome":"confirmed","platform_message_id":"fixture-message"})
    )}));
    assert!(observation(value.clone()).confirmed_terminal_text());
    for (pointer, replacement) in [
        ("/phase/response_id", json!("different-response")),
        ("/phase/durable_cursor", json!(0)),
        ("/delivery/terminal", json!(false)),
        ("/phase", json!({"state":"refused","code":"unavailable"})),
        ("/phase", json!({"state":"unknown"})),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert!(!observation(changed).confirmed_terminal_text());
    }
}

#[test]
fn routed_fixture_retains_source_custody_and_selects_one_conversation_consumer() {
    let mut value: Value =
        serde_json::from_str(include_str!("../../cmaeg-core/tests/fixtures/channel.json")).unwrap();
    value["sources"] = json!([{"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}}]);
    value["secrets"]["telegram_bot_token"] = "77:fixture".into();
    value["secrets"]["telegram_webhook_secret"] = "fixture-webhook".into();
    let original_source = value["sources"].clone();
    routed_channel(&mut value);
    assert_eq!(value["sources"], original_source);
    let mut definition: cmaeg_core::channel::ChannelDefinition =
        serde_json::from_value(value).unwrap();
    definition.profile_digest = definition.profile.digest();
    definition.validate().unwrap();
}
