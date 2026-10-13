//! `default_receives_all`: whether every event the policy admits reaches
//! `default`, condition by condition.
use cmaeg_core::channel::default_receives_all;
use serde_json::{json, Value};
use std::collections::BTreeSet;

use super::fixture::{definition, routed, REVIEWER};

fn providers(slugs: &[&str]) -> BTreeSet<String> {
    slugs.iter().map(|slug| (*slug).to_owned()).collect()
}

/// A routed channel where every condition holds: default is the only default
/// target, no other consumer can be addressed and default answers non-human
/// actors, being served by a configured non-human slug.
fn holding() -> Value {
    let mut value = routed();
    for consumer in value["routing"]["consumers"].as_array_mut().unwrap() {
        consumer["mentionable"] = false.into();
    }
    value["routing"]["addressing"] = json!({"event_field": false});
    value
}

fn receives_all(value: Value, slugs: &[&str]) -> bool {
    default_receives_all(&definition(value), &providers(slugs))
}

#[test]
fn a_channel_without_routing_sends_everything_to_default() {
    let mut value = routed();
    value.as_object_mut().unwrap().remove("routing");
    assert!(receives_all(value, &[]));
}

#[test]
fn every_condition_holding_means_default_receives_all() {
    assert!(receives_all(holding(), &["cma_codex"]));
}

#[test]
fn default_must_be_a_default_target() {
    let mut value = holding();
    value["routing"]["default_targets"] = json!([REVIEWER]);
    assert!(!receives_all(value.clone(), &["cma_codex"]));
    value["routing"]["default_targets"] = json!([REVIEWER, "default"]);
    assert!(receives_all(value, &["cma_codex"]));
}

#[test]
fn every_active_rule_must_target_default() {
    let mut value = holding();
    value["routing"]["rules"] =
        json!([{"id": "lark", "when": {"sources": ["relay_lark"]}, "targets": [REVIEWER]}]);
    assert!(!receives_all(value.clone(), &["cma_codex"]));
    // The same rule on a detached source is inert.
    value["routing"]["rules"][0]["when"]["sources"] = json!(["relay_slack"]);
    assert!(receives_all(value.clone(), &["cma_codex"]));
    value["routing"]["rules"] = json!([{"id": "all", "targets": [REVIEWER, "default"]}]);
    assert!(receives_all(value, &["cma_codex"]));
}

#[test]
fn a_mentionable_consumer_can_take_a_chat_message_from_default() {
    // Every rule includes default, but `reviewer` is mentionable.
    let mut value = holding();
    value["routing"]["rules"] = json!([{"id": "all", "targets": ["default", REVIEWER]}]);
    value["routing"]["consumers"][0]["mentionable"] = true.into();
    assert!(!receives_all(value.clone(), &["cma_codex"]));
    // Either directive is enough to address it.
    value["routing"]["addressing"] = json!({"mentions": false, "event_field": false});
    assert!(!receives_all(value.clone(), &["cma_codex"]));
    value["routing"]["addressing"] =
        json!({"mentions": false, "command": false, "event_field": false});
    assert!(receives_all(value.clone(), &["cma_codex"]));
    // The same channel with `mentionable: false`.
    value["routing"]["addressing"] = json!({"event_field": false});
    value["routing"]["consumers"][0]["mentionable"] = false.into();
    assert!(receives_all(value.clone(), &["cma_codex"]));
    // A pending consumer cannot be addressed yet; without a chat source, nothing can be.
    value["routing"]["consumers"][0]["mentionable"] = true.into();
    value["routing"]["consumers"][0]["pending"] = true.into();
    assert!(receives_all(value.clone(), &["cma_codex"]));
    value["routing"]["consumers"][0]["pending"] = false.into();
    value["sources"] = json!([value["sources"][2].clone()]);
    assert!(receives_all(value, &["cma_codex"]));
}

#[test]
fn an_event_field_can_take_a_custom_event_from_default() {
    // Custom attached, event_field on, another consumer.
    let mut value = holding();
    value["routing"]["addressing"] = json!({});
    assert!(!receives_all(value.clone(), &["cma_codex"]));
    for consumer in value["routing"]["consumers"].as_array_mut().unwrap() {
        consumer["pending"] = true.into();
    }
    assert!(receives_all(value.clone(), &["cma_codex"]));
    // Without a custom source the field addresses nothing.
    let mut chat_only = holding();
    chat_only["routing"]["addressing"] = json!({"mentions": true});
    chat_only["sources"] = json!([chat_only["sources"][0].clone()]);
    assert!(receives_all(chat_only, &[]));
}

#[test]
fn default_must_answer_non_human_actors_when_custom_is_attached() {
    // A nyxbot default with a custom source and no override.
    let mut value = holding();
    value["provider"]["slug"] = "nyxbot".into();
    assert!(!receives_all(value.clone(), &["cma_codex"]));
    value["routing"]["default_consumer"]["non_human"] = true.into();
    assert!(receives_all(value.clone(), &["cma_codex"]));
    value["routing"]["default_consumer"]["non_human"] = false.into();
    assert!(!receives_all(value.clone(), &["nyxbot"]));
    // Without a custom source no event needs a non-human answer.
    value["sources"] = json!([value["sources"][0].clone()]);
    assert!(receives_all(value, &[]));
}
