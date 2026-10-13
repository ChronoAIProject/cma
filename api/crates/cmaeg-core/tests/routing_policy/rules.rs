//! Routing rules and the policy-wide fan-out limits, each refusal pinned to
//! its exact pointer and message.
use serde_json::{json, Value};

use super::fixture::{accept, definition, expect, routed, NYX, REVIEWER};

fn rule(id: &str, targets: Value) -> Value {
    json!({"id": id, "when": {"sources": ["relay_lark"]}, "targets": targets})
}

#[test]
fn rules_are_bounded_unique_and_target_known_consumers() {
    expect(
        |value| {
            value["routing"]["rules"] = (0..33)
                .map(|index| rule(&format!("r{index}"), json!(["default"])))
                .collect();
        },
        "/routing/rules",
        "at most 32 rules",
    );
    expect(
        |value| {
            value["routing"]["rules"] =
                json!([rule("a", json!(["default"])), rule("a", json!([NYX]))])
        },
        "/routing/rules/1/id",
        "must be a unique rule id",
    );
    for targets in [
        json!([]),
        json!(["csm_0000000000ff"]),
        json!([NYX, NYX]),
        json!(["default", REVIEWER, NYX]),
    ] {
        expect(
            |value| {
                value["routing"]["max_targets"] = 2.into();
                value["routing"]["rules"] = json!([rule("a", targets)]);
            },
            "/routing/rules/0/targets",
            "must name 1 to max_targets known consumers",
        );
    }
    // A pending consumer is known, and a rule may name it.
    accept(|value| {
        value["routing"]["consumers"][1]["pending"] = true.into();
        value["routing"]["rules"] = json!([rule("a", json!([NYX]))]);
    });
}

#[test]
fn rule_match_lists_are_bounded() {
    let many: Vec<String> = (0..65).map(|index| format!("chat-{index}")).collect();
    for field in ["chats", "senders", "event_types", "event_sources"] {
        expect(
            |value| {
                value["routing"]["rules"] =
                    json!([{"id": "a", "when": {field: many}, "targets": ["default"]}])
            },
            &format!("/routing/rules/0/when/{field}"),
            "must contain at most 64 entries",
        );
    }
    let sources: Vec<&str> = std::iter::repeat_n("custom", 65).collect();
    expect(
        |value| {
            value["routing"]["rules"] =
                json!([{"id": "a", "when": {"sources": sources}, "targets": ["default"]}])
        },
        "/routing/rules/0/when/sources",
        "must contain at most 64 entries",
    );
    let kinds: Vec<&str> = std::iter::repeat_n("event", 65).collect();
    expect(
        |value| {
            value["routing"]["rules"] =
                json!([{"id": "a", "when": {"kinds": kinds}, "targets": ["default"]}])
        },
        "/routing/rules/0/when/kinds",
        "must contain at most 64 entries",
    );
    expect(
        |value| {
            value["routing"]["rules"] =
                json!([{"id": "a", "when": {"chats": ["c".repeat(513)]}, "targets": ["default"]}])
        },
        "/routing/rules/0/when/chats/0",
        "must contain at most 512 bytes",
    );
    expect(
        |value| {
            value["routing"]["rules"] =
                json!([{"id": "a", "when": {"senders": ["ou_1", ""]}, "targets": ["default"]}])
        },
        "/routing/rules/0/when/senders/1",
        "must contain at least 1 bytes",
    );
    accept(|value| {
        let full: Vec<String> = (0..64).map(|index| format!("{index:0>512}")).collect();
        value["routing"]["rules"] = json!([{"id": "a", "when": {"chats": full, "event_types": ["com.example.*"]}, "targets": ["default"]}]);
    });
}

#[test]
fn a_rule_naming_a_detached_source_is_valid_and_inert() {
    let mut value = routed();
    value["routing"]["rules"] = json!([
        {"id": "slack", "when": {"sources": ["relay_slack"]}, "targets": [NYX]},
        {"id": "lark", "when": {"sources": ["relay_slack", "relay_lark"]}, "targets": [REVIEWER]},
        {"id": "any", "targets": ["default"]}
    ]);
    let definition = definition(value);
    definition.validate().unwrap();
    let routing = definition.routing.as_ref().unwrap();
    let active: Vec<_> = routing
        .active_rules(&definition)
        .map(|rule| rule.id.as_str())
        .collect();
    assert_eq!(active, ["lark", "any"]);
}

#[test]
fn max_targets_is_one_to_eight() {
    for max in [0, 9] {
        expect(
            |value| value["routing"]["max_targets"] = max.into(),
            "/routing/max_targets",
            "must be 1 to 8",
        );
    }
    accept(|value| value["routing"]["max_targets"] = 8.into());
}

#[test]
fn agent_event_limits_are_bounded() {
    let pointer = "/routing/agent_events";
    for (field, values, message) in [
        ("max_hops", [0, 5], "must be 1 to 4"),
        ("per_consumer_per_minute", [0, 31], "must be 1 to 30"),
        ("per_channel_per_minute", [0, 61], "must be 1 to 60"),
    ] {
        for limit in values {
            expect(
                |value| value["routing"]["agent_events"] = json!({field: limit}),
                &format!("{pointer}/{field}"),
                message,
            );
        }
    }
    expect(
        |value| {
            value["routing"]["agent_events"] =
                json!({"per_consumer_per_minute": 20, "per_channel_per_minute": 10});
        },
        &format!("{pointer}/per_consumer_per_minute"),
        "must not exceed per_channel_per_minute",
    );
    accept(|value| {
        value["routing"]["agent_events"] =
            json!({"max_hops": 4, "per_consumer_per_minute": 30, "per_channel_per_minute": 60});
    });
}
