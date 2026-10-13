//! Routed definitions and activities the resolver tests share.
#![allow(dead_code)] // Each test binary that includes this uses a subset.
use std::{collections::BTreeSet, sync::OnceLock};

use cmaeg_core::{
    activity::{Activity, ActorKind, Causation, RouteResolution},
    channel::ChannelDefinition,
    routing::{resolve, Resolution, RouteDecision, RouteError, RouteInput, Trace},
};
use serde_json::{json, Value};

use super::policy::ROUTE;

/// The routed policy fixture with `default` presented as `main`: native
/// Telegram (`@golden_bot`), a Lark relay and a custom HMAC source, with
/// `reviewer` (cma_codex) and `nyx` (nyxbot) beside `default`.
pub fn routed() -> Value {
    let mut value = super::policy::routed();
    value["routing"]["default_consumer"]["name"] = "main".into();
    value
}

/// The routed fixture with `edit` applied; it must still be a valid definition.
pub fn channel(edit: impl FnOnce(&mut Value)) -> ChannelDefinition {
    let definition = unchecked(edit);
    definition.validate().unwrap();
    definition
}

/// The routed fixture with `edit` applied, as stored, without the reader's
/// rules: for policies a reader would refuse.
pub fn unchecked(edit: impl FnOnce(&mut Value)) -> ChannelDefinition {
    let mut value = routed();
    edit(&mut value);
    serde_json::from_value(value).unwrap()
}

/// The routed fixture with routing removed: a legacy channel.
pub fn unrouted() -> ChannelDefinition {
    let mut value = routed();
    value.as_object_mut().unwrap().remove("routing");
    value["secrets"]
        .as_object_mut()
        .unwrap()
        .remove("consumer_keys");
    let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.validate().unwrap();
    definition
}

pub fn rule(id: &str, when: Value, targets: Value) -> Value {
    json!({"id": id, "when": when, "targets": targets})
}

pub fn telegram() -> Value {
    json!({"type": "telegram", "platform": "telegram", "bot_id": "77"})
}

pub fn lark() -> Value {
    json!({"type": "nyxid_relay", "platform": "lark", "route_id": ROUTE})
}

pub fn custom() -> Value {
    json!({"type": "custom"})
}

fn activity(source: Value, kind: Value, actor: &str) -> Activity {
    let chat = if source["type"] == "custom" {
        "custom"
    } else {
        "group"
    };
    serde_json::from_value(json!({
        "schema_version": 1, "event_id": "evt-1", "kind": kind,
        "actor": {"id": "user-1", "kind": actor},
        "conversation": {"id": "chat-1", "kind": chat},
        "occurred_at": "2026-10-01T00:00:00Z", "source": source
    }))
    .unwrap()
}

/// A human's chat message.
pub fn message(source: Value, text: &str) -> Activity {
    activity(
        source,
        json!({"type": "message", "text": text, "mentions_bot": true}),
        "human",
    )
}

/// A human's chat command.
pub fn command(source: Value, name: &str, args: &str) -> Activity {
    activity(
        source,
        json!({"type": "command", "name": name, "args": args}),
        "human",
    )
}

/// A card click by a human.
pub fn click(source: Value) -> Activity {
    activity(
        source,
        json!({"type": "interaction", "block_id": "b-1", "action_id": "approve"}),
        "human",
    )
}

/// A custom CloudEvent from a `system` actor, with `extensions` merged in.
pub fn event(event_type: &str, extensions: Value) -> Activity {
    let mut ce = json!({"specversion": "1.0", "id": "ce-1", "source": "https://ci.example/build",
        "type": event_type});
    for (name, value) in extensions.as_object().unwrap() {
        ce[name] = value.clone();
    }
    activity(custom(), json!({"type": "event", "ce": ce}), "system")
}

/// Provider slugs that answer non-human actors: `cma_codex` (so `default`
/// and `reviewer`) but not `nyxbot`.
pub fn non_human_providers() -> &'static BTreeSet<String> {
    static SET: OnceLock<BTreeSet<String>> = OnceLock::new();
    SET.get_or_init(|| BTreeSet::from(["cma_codex".to_owned()]))
}

/// A signed, non-click, non-hand-off input for `activity` on `def`.
pub fn input<'a>(def: &'a ChannelDefinition, activity: &'a Activity) -> RouteInput<'a> {
    RouteInput {
        activity,
        chat_source: def.source_of(&activity.source),
        card_consumer: None,
        published: None,
        non_human_providers: non_human_providers(),
        attributes_signed: true,
    }
}

pub fn trace() -> Trace {
    Trace::default()
}

/// What a routed outcome names, for compact assertions: the resolution and
/// the target ids in order.
pub fn routed_to(
    outcome: Result<Resolution, RouteError>,
) -> Result<Option<(RouteResolution, Vec<String>)>, RouteError> {
    Ok(match outcome? {
        Resolution::Legacy => None,
        Resolution::Routed(decision) => Some((
            decision.routing.resolution,
            decision
                .routing
                .targets
                .iter()
                .map(|target| target.id.to_string())
                .collect(),
        )),
    })
}

/// The routed decision `resolve` returns with routing enabled.
pub fn decision(def: &ChannelDefinition, input: &RouteInput<'_>) -> RouteDecision {
    match resolve(def, input, true) {
        Ok(Resolution::Routed(decision)) => *decision,
        other => panic!("expected a routed decision, got {other:?}"),
    }
}

/// A hand-off's causation, published by `reviewer`.
pub fn causation() -> Causation {
    serde_json::from_value(json!({"parent_event_id": "evt-0",
        "root": "0123456789abcdef0123456789abcdef", "published_by": super::policy::REVIEWER,
        "hop": 1, "reply": "origin", "origin_source_id": "telegram"}))
    .unwrap()
}

/// A hand-off: a `bot` actor's message, as the publishing consumer wrote it.
pub fn handoff(text: &str) -> Activity {
    let mut activity = message(telegram(), text);
    activity.actor.kind = ActorKind::Bot;
    activity
}
