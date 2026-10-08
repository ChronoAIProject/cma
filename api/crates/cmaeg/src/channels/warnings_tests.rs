use super::*;
use crate::channels::routing_fixture::{self as fixture, NYX, REVIEWER};
use serde_json::{json, Value};

fn check(edit: impl FnOnce(&mut Value), non_human: &[&str], tools: &[&str]) -> Vec<Warning> {
    let mut raw = fixture::routed();
    edit(&mut raw);
    let definition = fixture::definition(raw);
    definition.validate().unwrap();
    for_channel(
        &definition,
        &non_human.iter().map(|value| (*value).into()).collect(),
        &tools.iter().map(|value| value.parse().unwrap()).collect(),
    )
}
fn warning(pointer: &str, reason: WarningReason) -> Warning {
    Warning {
        pointer: pointer.into(),
        reason,
    }
}

#[test]
fn custom_targets_report_effective_capability_at_each_target_pointer() {
    let warnings = check(
        |raw| {
            raw["routing"]["default_consumer"]["non_human"] = false.into();
            raw["routing"]["rules"] = json!([{"id":"custom","when":{"sources":["custom"],"kinds":["event"]},"targets":[REVIEWER]}]);
        },
        &["cma_codex"],
        &[],
    );
    assert_eq!(
        warnings,
        [warning(
            "/routing/default_targets/0",
            WarningReason::TargetIgnoresCustomEvents
        )]
    );
    let warnings = check(
        |raw| {
            raw["routing"]["default_consumer"]["non_human"] = true.into();
            raw["routing"]["rules"] =
                json!([{"id":"custom","when":{"sources":["custom"]},"targets":[REVIEWER]}]);
        },
        &[],
        &[],
    );
    assert_eq!(
        warnings,
        [warning(
            "/routing/rules/0/targets/0",
            WarningReason::TargetIgnoresCustomEvents
        )]
    );
}

#[test]
fn custom_warnings_exclude_chat_only_kinds_and_other_or_detached_sources() {
    for when in [
        json!({"kinds":["message"]}),
        json!({"sources":["relay_lark"]}),
        json!({"sources":["relay_feishu"]}),
    ] {
        let warnings = check(
            |raw| {
                raw["routing"]["default_consumer"]["non_human"] = true.into();
                raw["routing"]["consumers"][0]["mentionable"] = false.into();
                raw["routing"]["consumers"][1]["mentionable"] = false.into();
                raw["routing"]["rules"] =
                    json!([{"id":"scoped","when":when,"targets":["default",REVIEWER]}]);
            },
            &[],
            &[],
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }
    assert!(check(
        |raw| {
            raw["sources"]
                .as_array_mut()
                .unwrap()
                .retain(|source| source["type"] != "custom");
        },
        &[],
        &[]
    )
    .is_empty());
}

#[test]
fn explicit_publish_targets_warn_even_when_disabled_and_tools_only_when_enabled() {
    let warnings = check(
        |raw| {
            raw["routing"]["default_consumer"]["publish"] =
                json!({"enabled":false,"targets":[NYX]});
            raw["routing"]["consumers"][0]["publish"] = json!({"enabled":true,"targets":[NYX]});
        },
        &["cma_codex"],
        &[],
    );
    assert_eq!(
        warnings,
        [
            warning(
                "/routing/default_consumer/publish/targets/0",
                WarningReason::PublishTargetIgnoresAgentEvents
            ),
            warning(
                "/routing/consumers/0/publish/enabled",
                WarningReason::PublishToolsUnavailable
            ),
            warning(
                "/routing/consumers/0/publish/targets/0",
                WarningReason::PublishTargetIgnoresAgentEvents
            ),
        ]
    );
    assert!(check(
        |raw| {
            raw["routing"]["consumers"][0]["publish"] = json!({"enabled":true,"targets":[NYX]});
            raw["routing"]["consumers"][1]["non_human"] = true.into();
        },
        &["cma_codex"],
        &["cma_codex"]
    )
    .is_empty());
}

#[test]
fn publish_wildcards_consider_only_other_ready_consumers_when_enabled() {
    for enabled in [false, true] {
        let warnings = check(
            |raw| {
                raw["routing"]["default_consumer"]["publish"] =
                    json!({"enabled":enabled,"targets":[]});
            },
            &["cma_codex"],
            &["cma_codex"],
        );
        assert_eq!(
            warnings,
            if enabled {
                vec![warning(
                    "/routing/default_consumer/publish/targets",
                    WarningReason::PublishTargetIgnoresAgentEvents,
                )]
            } else {
                vec![]
            }
        );
    }
    assert!(check(
        |raw| {
            raw["routing"]["default_consumer"]["publish"] = json!({"enabled":true,"targets":[]});
            raw["routing"]["consumers"][1]["pending"] = true.into();
        },
        &["cma_codex"],
        &["cma_codex"]
    )
    .is_empty());
}

fn bypass(raw: &mut Value, when: Value) {
    raw["routing"]["consumers"][1]["mentionable"] = false.into();
    raw["routing"]["rules"] = json!([{"id":"limited","when":when,"targets":["default"]}]);
}
#[test]
fn addressing_warns_once_for_a_ready_consumer_excluded_from_a_chat_rule() {
    let warnings = check(
        |raw| bypass(raw, json!({"sources":["relay_lark"],"kinds":["message"]})),
        &["cma_codex", "nyxbot"],
        &[],
    );
    assert_eq!(
        warnings,
        [warning(
            "/routing/consumers/0/mentionable",
            WarningReason::MentionableBypassesRules
        )]
    );
    for change in ["pending", "hidden", "addressing_off", "no_chat"] {
        assert!(
            check(
                |raw| {
                    bypass(raw, json!({}));
                    match change {
                        "pending" => raw["routing"]["consumers"][0]["pending"] = true.into(),
                        "hidden" => raw["routing"]["consumers"][0]["mentionable"] = false.into(),
                        "addressing_off" => {
                            raw["routing"]["addressing"] = json!({"mentions":false,"command":false})
                        }
                        "no_chat" => {
                            raw["sources"]
                                .as_array_mut()
                                .unwrap()
                                .retain(|source| source["type"] == "custom");
                            raw["reply"]["sinks"] = json!([{"type":"sync"}]);
                        }
                        _ => unreachable!(),
                    }
                },
                &["cma_codex", "nyxbot"],
                &[]
            )
            .is_empty(),
            "{change}"
        );
    }
}

#[test]
fn addressing_excludes_inert_event_and_interaction_rules_and_respects_command_switch() {
    for when in [
        json!({"sources":["relay_feishu"]}),
        json!({"kinds":["event"]}),
        json!({"kinds":["interaction"]}),
        json!({"event_types":["test.*"]}),
        json!({"event_sources":["https://source.example/*"]}),
    ] {
        assert!(check(|raw| bypass(raw, when), &["cma_codex", "nyxbot"], &[]).is_empty());
    }
    for commands in [false, true] {
        let warnings = check(
            |raw| {
                bypass(raw, json!({"kinds":["command"]}));
                raw["routing"]["addressing"] = json!({"mentions":true,"command":commands});
            },
            &["cma_codex", "nyxbot"],
            &[],
        );
        assert_eq!(warnings.len(), usize::from(commands));
    }
}

#[test]
fn fanout_warns_for_both_custom_response_modes_without_an_output_destination() {
    for by_rule in [false, true] {
        let warnings = check(
            |raw| {
                raw["reply"]["sinks"] = json!([{"type":"sync"}]);
                if by_rule {
                    raw["routing"]["rules"] = json!([{"id":"custom","when":{"sources":["custom"]},"targets":["default",REVIEWER]}]);
                } else {
                    raw["routing"]["default_targets"] = json!(["default", REVIEWER]);
                }
            },
            &["cma_codex", "nyxbot"],
            &[],
        );
        let pointer = if by_rule {
            "/routing/rules/0/targets"
        } else {
            "/routing/default_targets"
        };
        assert_eq!(
            warnings,
            [
                warning(pointer, WarningReason::FanoutRequiresStreamAll),
                warning(pointer, WarningReason::FanoutOutputsUndeliverable)
            ]
        );
    }
    for fallback in ["webhook", "default_reply"] {
        assert!(check(
            |raw| {
                raw["routing"]["default_targets"] = json!(["default", REVIEWER]);
                if fallback == "default_reply" {
                    raw["reply"]["sinks"] = json!([{"type":"sync"}]);
                    raw["routing"]["default_reply"] =
                        json!({"type":"telegram_chat","source_id":"telegram","chat_id":"42"});
                }
            },
            &["cma_codex", "nyxbot"],
            &[]
        )
        .is_empty());
    }
}

#[test]
fn legacy_definitions_and_fully_supported_policy_have_no_warnings() {
    assert!(check(|_| {}, &["cma_codex", "nyxbot"], &[]).is_empty());
    assert!(check(
        |raw| {
            raw.as_object_mut().unwrap().remove("routing");
            raw["secrets"]
                .as_object_mut()
                .unwrap()
                .remove("consumer_keys");
        },
        &[],
        &[]
    )
    .is_empty());
}

#[test]
fn pending_publishers_keep_static_warnings_without_addressing_warnings() {
    let warnings = check(
        |raw| {
            bypass(raw, json!({"kinds":["message"]}));
            raw["routing"]["default_consumer"]["non_human"] = true.into();
            raw["routing"]["consumers"][0]["pending"] = true.into();
            raw["routing"]["consumers"][0]["publish"] = json!({"enabled":true,"targets":[NYX]});
        },
        &[],
        &[],
    );
    assert_eq!(
        warnings,
        [
            warning(
                "/routing/consumers/0/publish/enabled",
                WarningReason::PublishToolsUnavailable
            ),
            warning(
                "/routing/consumers/0/publish/targets/0",
                WarningReason::PublishTargetIgnoresAgentEvents
            ),
        ]
    );
}
