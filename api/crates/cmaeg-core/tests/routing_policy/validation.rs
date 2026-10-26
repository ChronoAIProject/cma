//! Every reader rule of a routing policy, each refusal pinned to its exact
//! pointer and message.
use serde_json::{json, Value};

use super::fixture::{
    accept, agent_key, consumer, definition, expect, hash, routed, NYX, REVIEWER,
};

fn first(value: &mut Value) -> &mut Value {
    &mut value["routing"]["consumers"][0]
}

fn borrow_agent(target: &mut Value, agent: &str) {
    target["target"] = json!({"type": "existing_agent", "agent_id": agent});
}

const NAME: &str = "must be a unique, unreserved consumer name";
const LABEL: &str = "must be 1 to 64 printable characters";
const KEYS: &str = "must hold one key per consumer";
const DEFAULT_TARGETS: &str = "must name 1 to max_targets ready consumers";

#[test]
fn the_routed_fixture_is_a_valid_definition() {
    let routed = definition(routed());
    routed.validate().unwrap();
    // The legacy rules never look at routing.
    routed.validate_legacy().unwrap();
}

#[test]
fn the_version_must_be_one() {
    expect(
        |value| value["routing"]["version"] = 2.into(),
        "/routing/version",
        "unsupported version",
    );
}

#[test]
fn at_most_seven_additional_consumers() {
    expect(
        |value| {
            value["routing"]["consumers"] = (0..8)
                .map(|index| consumer(&format!("csm_00000000010{index}"), "extra", "cma_codex"))
                .collect();
        },
        "/routing/consumers",
        "at most seven additional consumers",
    );
}

#[test]
fn consumer_ids_are_generated_and_unique() {
    let unique = "must be a unique consumer id";
    expect(
        |value| first(value)["id"] = "default".into(),
        "/routing/consumers/0/id",
        unique,
    );
    expect(
        |value| value["routing"]["consumers"][1]["id"] = REVIEWER.into(),
        "/routing/consumers/1/id",
        unique,
    );
}

#[test]
fn names_are_unique_unreserved_and_not_a_bot() {
    for name in [
        "all",
        "everyone",
        "here",
        "to",
        "csm_reviewer",
        "default",
        "lead",
    ] {
        expect(
            |value| first(value)["name"] = name.into(),
            "/routing/consumers/0/name",
            NAME,
        );
    }
    expect(
        |value| value["routing"]["consumers"][1]["name"] = "reviewer".into(),
        "/routing/consumers/1/name",
        NAME,
    );
    for name in ["golden_bot", "lark_bot"] {
        expect(
            |value| first(value)["name"] = name.into(),
            "/routing/consumers/0/name",
            NAME,
        );
    }
    // Bot usernames compare case-insensitively.
    expect(
        |value| {
            value["sources"][0]["bot_username"] = "Reviewer_Bot".into();
            first(value)["name"] = "reviewer_bot".into();
        },
        "/routing/consumers/0/name",
        NAME,
    );
    for name in ["here", "csm_x", "golden_bot"] {
        expect(
            |value| value["routing"]["default_consumer"]["name"] = name.into(),
            "/routing/default_consumer/name",
            NAME,
        );
    }
    // Only the default consumer may be named default, and it need not be.
    accept(|value| value["routing"]["default_consumer"]["name"] = "default".into());
}

#[test]
fn labels_are_short_and_printable() {
    for label in [
        String::new(),
        "x".repeat(65),
        "bell\u{7}".into(),
        "line\nbreak".into(),
        "\u{202E}weiver".into(),
        "iso\u{2066}late".into(),
        "mark\u{200F}".into(),
    ] {
        expect(
            |value| first(value)["label"] = label.clone().into(),
            "/routing/consumers/0/label",
            LABEL,
        );
        expect(
            |value| value["routing"]["default_consumer"]["label"] = label.into(),
            "/routing/default_consumer/label",
            LABEL,
        );
    }
    accept(|value| first(value)["label"] = "é".repeat(64).into());
    accept(|value| first(value)["label"] = "Code Reviewer 🤖".into());
}

#[test]
fn a_consumer_profile_is_valid_digested_and_served_by_its_provider() {
    expect(
        |value| first(value)["profile"]["metadata"]["name"] = "".into(),
        "/routing/consumers/0/profile/metadata/name",
        "must contain at least 1 bytes",
    );
    expect(
        |value| first(value)["profile_digest"] = hash("another profile").into(),
        "/routing/consumers/0/profile_digest",
        "does not match profile",
    );
    expect(
        |value| first(value)["provider"]["slug"] = "nyxbot".into(),
        "/routing/consumers/0/provider/slug",
        "must match the profile provider",
    );
    expect(
        |value| borrow_agent(&mut first(value)["provider"], "agent one"),
        "/routing/consumers/0/provider/target/agent_id",
        "must be a provider agent identifier",
    );
}

#[test]
fn agent_key_hashes_are_wellformed_and_never_shared() {
    expect(
        |value| first(value)["agent_key_hash"] = "sha256:ABC".into(),
        "/routing/consumers/0/agent_key_hash",
        "must be sha256: followed by lowercase hex",
    );
    expect(
        |value| first(value)["agent_key_hash"] = hash(&agent_key("default")).into(),
        "/routing/consumers/0/agent_key_hash",
        "consumer_key_reused",
    );
    expect(
        |value| {
            value["routing"]["consumers"][1]["agent_key_hash"] = hash(&agent_key(REVIEWER)).into()
        },
        "/routing/consumers/1/agent_key_hash",
        "consumer_key_reused",
    );
}

#[test]
fn an_existing_agent_serves_one_consumer() {
    expect(
        |value| {
            borrow_agent(&mut value["provider"], "agt_shared");
            borrow_agent(&mut first(value)["provider"], "agt_shared");
        },
        "/routing/consumers/0/provider/target",
        "duplicate_agent_target",
    );
    expect(
        |value| {
            borrow_agent(&mut first(value)["provider"], "agt_shared");
            borrow_agent(
                &mut value["routing"]["consumers"][1]["provider"],
                "agt_shared",
            );
        },
        "/routing/consumers/1/provider/target",
        "duplicate_agent_target",
    );
    accept(|value| {
        borrow_agent(&mut value["provider"], "agt_default");
        borrow_agent(&mut first(value)["provider"], "agt_reviewer");
    });
}

#[test]
fn existing_agents_require_automatic_finalization() {
    let message = "existing agents require auto_final";
    expect(
        |value| {
            borrow_agent(&mut first(value)["provider"], "agt_reviewer");
            first(value)["reply"] = json!({"auto_final": false});
        },
        "/routing/consumers/0/reply/auto_final",
        message,
    );
    // Without an override the channel's policy applies.
    expect(
        |value| {
            value["reply"]["auto_final"] = false.into();
            borrow_agent(&mut first(value)["provider"], "agt_reviewer");
        },
        "/routing/consumers/0/reply/auto_final",
        message,
    );
    accept(|value| {
        value["reply"]["auto_final"] = false.into();
        borrow_agent(&mut first(value)["provider"], "agt_reviewer");
        first(value)["reply"] = json!({"auto_final": true});
    });
    expect(
        |value| {
            borrow_agent(&mut value["provider"], "agt_default");
            value["routing"]["default_consumer"]["reply"] = json!({"auto_final": false});
        },
        "/routing/default_consumer/reply/auto_final",
        message,
    );
}

#[test]
fn publishing_needs_a_profile_target_and_other_consumers() {
    let publish = json!({"enabled": true});
    expect(
        |value| {
            borrow_agent(&mut first(value)["provider"], "agt_reviewer");
            first(value)["publish"] = publish.clone();
        },
        "/routing/consumers/0/publish/enabled",
        "publishing requires a profile target",
    );
    expect(
        |value| {
            borrow_agent(&mut value["provider"], "agt_default");
            value["routing"]["default_consumer"]["publish"] = publish.clone();
        },
        "/routing/default_consumer/publish/enabled",
        "publishing requires a profile target",
    );
    let other = "must name another consumer";
    for (target, index) in [(REVIEWER, 1), ("csm_0000000000ff", 1)] {
        expect(
            |value| first(value)["publish"] = json!({"enabled": true, "targets": [NYX, target]}),
            &format!("/routing/consumers/0/publish/targets/{index}"),
            other,
        );
    }
    for target in ["default", "csm_0000000000ff"] {
        expect(
            |value| {
                value["routing"]["default_consumer"]["publish"] =
                    json!({"enabled": true, "targets": [target]});
            },
            "/routing/default_consumer/publish/targets/0",
            other,
        );
    }
    accept(|value| {
        first(value)["publish"] = json!({"enabled": true, "targets": ["default", NYX]});
        value["routing"]["default_consumer"]["publish"] =
            json!({"enabled": true, "targets": [REVIEWER]});
    });
}

#[test]
fn consumer_keys_hold_one_bounded_key_per_consumer() {
    for keys in [
        json!([[REVIEWER, agent_key(REVIEWER)]]),
        json!([
            [REVIEWER, agent_key(REVIEWER)],
            [NYX, agent_key(NYX)],
            ["csm_0000000000ff", "nyx_extra"]
        ]),
        json!([
            [REVIEWER, agent_key(REVIEWER)],
            [REVIEWER, agent_key(REVIEWER)],
            [NYX, agent_key(NYX)]
        ]),
        json!([[REVIEWER, ""], [NYX, agent_key(NYX)]]),
        json!([[REVIEWER, "nyx key"], [NYX, agent_key(NYX)]]),
        json!([[REVIEWER, "k".repeat(4097)], [NYX, agent_key(NYX)]]),
    ] {
        expect(
            |value| value["secrets"]["consumer_keys"] = keys,
            "/secrets/consumer_keys",
            KEYS,
        );
    }
    accept(|value| {
        value["secrets"]["consumer_keys"] =
            json!([[NYX, "k".repeat(4096)], [REVIEWER, agent_key(REVIEWER)]]);
    });
    // Keys without a routing policy have no consumer to belong to.
    expect(
        |value| {
            value.as_object_mut().unwrap().remove("routing");
        },
        "/secrets/consumer_keys",
        KEYS,
    );
}

#[test]
fn default_targets_name_one_to_max_targets_ready_consumers() {
    for targets in [
        json!([]),
        json!(["default", "default"]),
        json!(["csm_0000000000ff"]),
        json!(["default", REVIEWER, NYX]),
    ] {
        expect(
            |value| {
                value["routing"]["max_targets"] = 2.into();
                value["routing"]["default_targets"] = targets;
            },
            "/routing/default_targets",
            DEFAULT_TARGETS,
        );
    }
    expect(
        |value| {
            value["routing"]["consumers"][1]["pending"] = true.into();
            value["routing"]["default_targets"] = json!([NYX]);
        },
        "/routing/default_targets",
        DEFAULT_TARGETS,
    );
    accept(|value| value["routing"]["default_targets"] = json!([REVIEWER, NYX, "default"]));
}

#[test]
fn consumers_record_when_they_were_added() {
    expect(
        |value| first(value)["created_at"] = "yesterday".into(),
        "/routing/consumers/0/created_at",
        "must be RFC 3339",
    );
}
