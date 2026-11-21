//! The routing decision bound into a routed activity: its wire shape, and the
//! checks `Activity::validate` applies to it.
use cmaeg_core::{
    activity::{Activity, RouteResolution},
    seal::event_ref::activity_digest,
};
use serde_json::{json, Value};

const ROOT: &str = "0123456789abcdef0123456789abcdef";

fn message(text: &str) -> Value {
    let mut value: Value = serde_json::from_str(include_str!("fixtures/activity.json")).unwrap();
    value["kind"]["text"] = json!(text);
    value
}

fn target(id: &str, name: &str) -> Value {
    json!({"id": id, "name": name})
}

/// A rule-routed message to two consumers, `@reviewer` addressed.
fn routed() -> Value {
    let mut value = message("@reviewer please check");
    value["routing"] = json!({
        "source_id": "relay_telegram",
        "resolution": "rule",
        "rule_id": "triage",
        "targets": [target("csm_00000000000a", "reviewer"), target("default", "assistant")],
        "addressing_bytes": 10
    });
    value
}

/// A hand-off published by a consumer, with its causation.
fn handoff() -> Value {
    let mut value = message("summarize this");
    value["routing"] = json!({
        "source_id": "agent",
        "resolution": "agent_event",
        "targets": [target("csm_00000000000b", "writer")],
        "causation": {
            "parent_event_id": "msg-0",
            "root": ROOT,
            "published_by": "csm_00000000000a",
            "hop": 1,
            "reply": "origin",
            "origin_source_id": "relay_telegram"
        }
    });
    value
}

fn parse(value: Value) -> Activity {
    serde_json::from_value(value).unwrap()
}

fn refusal(value: Value) -> String {
    parse(value).validate().unwrap_err().pointer
}

#[test]
fn routed_activities_round_trip_every_field() {
    for value in [routed(), handoff()] {
        let activity = parse(value.clone());
        activity.validate().unwrap();
        assert_eq!(serde_json::to_value(&activity).unwrap(), value);
        let reparsed: Activity =
            serde_json::from_str(&serde_json::to_string(&activity).unwrap()).unwrap();
        assert_eq!(reparsed, activity);
    }
    let routing = parse(handoff()).routing.unwrap();
    assert_eq!(routing.resolution, RouteResolution::AgentEvent);
    assert_eq!(routing.causation.unwrap().hop, 1);
}

#[test]
fn routing_is_bound_into_the_activity_digest_and_absent_routing_is_omitted() {
    let unrouted = parse(message("@reviewer please check"));
    let serialized = serde_json::to_string(&unrouted).unwrap();
    assert!(!serialized.contains("routing"));
    assert_ne!(
        activity_digest(&unrouted),
        activity_digest(&parse(routed()))
    );
    // Zero addressing is the default and is omitted, keeping the bytes stable.
    let mut value = routed();
    value["routing"]["addressing_bytes"] = json!(0);
    let encoded = serde_json::to_value(parse(value)).unwrap();
    assert!(encoded["routing"].get("addressing_bytes").is_none());
}

#[test]
fn every_resolution_has_its_snake_case_wire_name() {
    for name in [
        "default",
        "rule",
        "mention",
        "command",
        "event_field",
        "interaction",
        "agent_event",
    ] {
        let resolution: RouteResolution = serde_json::from_value(json!(name)).unwrap();
        assert_eq!(serde_json::to_value(resolution).unwrap(), json!(name));
    }
    assert!(serde_json::from_value::<RouteResolution>(json!("broadcast")).is_err());
}

#[test]
fn routing_refuses_unknown_fields_and_malformed_ids() {
    for (pointer, value) in [
        ("/routing/extra", json!(true)),
        ("/routing/targets/0/label", json!("Reviewer")),
        ("/routing/targets/0/id", json!("reviewer")),
        ("/routing/source_id", json!("telegram-new")),
        ("/routing/rule_id", json!("Triage")),
    ] {
        let mut routed = routed();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        routed.pointer_mut(parent).unwrap()[key] = value;
        assert!(
            serde_json::from_value::<Activity>(routed).is_err(),
            "{pointer}"
        );
    }
    let mut value = handoff();
    value["routing"]["causation"]["extra"] = json!(1);
    assert!(serde_json::from_value::<Activity>(value).is_err());
}

#[test]
fn targets_are_one_to_eight_unique_consumers() {
    let mut value = routed();
    value["routing"]["targets"] = json!([]);
    assert_eq!(refusal(value), "/routing/targets");

    let mut value = routed();
    value["routing"]["targets"] = json!([
        target("csm_00000000000a", "reviewer"),
        target("csm_00000000000a", "reviewer-two")
    ]);
    assert_eq!(refusal(value), "/routing/targets");

    let eight = (0..8)
        .map(|index| {
            target(
                &format!("csm_00000000000{index}"),
                &format!("agent-{index}"),
            )
        })
        .collect::<Vec<_>>();
    let mut value = routed();
    value["routing"]["targets"] = json!(eight.clone());
    parse(value.clone()).validate().unwrap();
    let mut nine = eight;
    nine.push(target("csm_000000000009", "agent-9"));
    value["routing"]["targets"] = json!(nine);
    assert_eq!(refusal(value), "/routing/targets");
}

#[test]
fn addressing_stays_within_the_message_text_on_a_character_boundary() {
    let mut value = routed();
    value["routing"]["addressing_bytes"] = json!("@reviewer please check".len());
    parse(value.clone()).validate().unwrap();
    value["routing"]["addressing_bytes"] = json!("@reviewer please check".len() + 1);
    assert_eq!(refusal(value), "/routing/addressing_bytes");

    let mut value = message("@réviewer hi");
    value["routing"] = routed()["routing"].clone();
    value["routing"]["addressing_bytes"] = json!(3);
    assert_eq!(refusal(value.clone()), "/routing/addressing_bytes");
    value["routing"]["addressing_bytes"] = json!(4);
    parse(value).validate().unwrap();
}

#[test]
fn addressing_counts_into_to_command_arguments_and_is_zero_elsewhere() {
    let mut command = routed();
    command["routing"]["resolution"] = json!("command");
    command["kind"] = json!({"type": "command", "name": "to", "args": "reviewer please"});
    command["routing"]["addressing_bytes"] = json!(9);
    parse(command.clone()).validate().unwrap();
    command["routing"]["addressing_bytes"] = json!(16);
    assert_eq!(refusal(command.clone()), "/routing/addressing_bytes");

    command["kind"] = json!({"type": "command", "name": "help", "args": "reviewer please"});
    command["routing"]["addressing_bytes"] = json!(9);
    assert_eq!(refusal(command.clone()), "/routing/addressing_bytes");
    command["routing"]["addressing_bytes"] = json!(0);
    parse(command).validate().unwrap();

    let mut interaction = routed();
    interaction["kind"] = json!({"type": "interaction", "block_id": "b", "action_id": "a"});
    assert_eq!(refusal(interaction.clone()), "/routing/addressing_bytes");
    interaction["routing"]
        .as_object_mut()
        .unwrap()
        .remove("addressing_bytes");
    parse(interaction).validate().unwrap();
}

#[test]
fn causation_is_present_exactly_on_agent_hand_offs() {
    let mut value = handoff();
    value["routing"]
        .as_object_mut()
        .unwrap()
        .remove("causation");
    assert_eq!(refusal(value), "/routing/causation");

    let mut value = handoff();
    value["routing"]["resolution"] = json!("rule");
    assert_eq!(refusal(value), "/routing/causation");

    let mut value = handoff();
    value["routing"]["source_id"] = json!("custom");
    assert_eq!(refusal(value), "/routing/causation");

    let mut value = routed();
    value["routing"]["source_id"] = json!("agent");
    assert_eq!(refusal(value), "/routing/causation");

    let mut value = routed();
    value["routing"]["causation"] = handoff()["routing"]["causation"].clone();
    assert_eq!(refusal(value), "/routing/causation");
}

#[test]
fn causation_bounds_its_hop_and_root() {
    for hop in [0, 5] {
        let mut value = handoff();
        value["routing"]["causation"]["hop"] = json!(hop);
        assert_eq!(refusal(value), "/routing/causation/hop");
    }
    let mut value = handoff();
    value["routing"]["causation"]["hop"] = json!(4);
    parse(value).validate().unwrap();
    for root in [
        ROOT.to_uppercase(),
        ROOT[..31].to_owned(),
        format!("{ROOT}0"),
        "g".repeat(32),
    ] {
        let mut value = handoff();
        value["routing"]["causation"]["root"] = json!(root);
        assert_eq!(refusal(value), "/routing/causation/root");
    }
}
