//! The gate's rules on constructed definitions. No storage is involved: the
//! carry-through and removal rules are decided from the two definitions alone.
use super::{check, WriteFlags};
use cmaeg_core::{
    channel::{ChannelDefinition, Secret},
    profile::EventWorkerProfile,
    ValidationError,
};
use serde_json::{json, Value};

fn relay(platform: &str, route: &str) -> Value {
    json!({"type":"nyxid_relay","issuer":"https://nyx.example",
        "jwks_url":"https://nyx.example/.well-known/jwks.json","key_id":"key-1",
        "route_ids":[route],"platform":platform,"admission":{"type":"all"}})
}

fn definition(sources: Value) -> ChannelDefinition {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    value["sources"] = sources;
    value["partition"]["key"] = "conversation_and_sender".into();
    value["secrets"]["channel_key"] = "gate-channel-key".into();
    let profile: EventWorkerProfile = serde_json::from_value(value["profile"].clone()).unwrap();
    value["profile_digest"] = profile.digest().into();
    serde_json::from_value(value).unwrap()
}

fn legacy() -> ChannelDefinition {
    definition(json!([relay("lark", "route-lark")]))
}

/// What a writer behind the flag would have stored: two relays.
fn extended() -> ChannelDefinition {
    definition(json!([
        relay("lark", "route-lark"),
        relay("feishu", "route-feishu")
    ]))
}

fn duplicated(index: usize) -> ValidationError {
    ValidationError::new(format!("/sources/{index}"), "source variant is duplicated")
}

#[test]
fn legacy_writes_are_allowed() {
    let next = legacy();
    next.validate_legacy().unwrap();
    check(None, &next, WriteFlags::default()).unwrap();
    let mut renamed = legacy();
    renamed.name = "renamed".into();
    check(Some(&next), &renamed, WriteFlags::default()).unwrap();
}

#[test]
fn introducing_a_second_relay_gets_the_legacy_error() {
    let next = extended();
    assert_eq!(
        check(None, &next, WriteFlags::default()),
        Err(duplicated(1))
    );
    assert_eq!(
        check(Some(&legacy()), &next, WriteFlags::default()),
        Err(duplicated(1))
    );
    assert_eq!(next.validate_legacy(), Err(duplicated(1)));
}

#[test]
fn an_unchanged_extension_is_carried_through_with_the_flag_off() {
    let current = extended();
    let mut next = extended();
    next.name = "renamed".into();
    next.reply.ack_after_ms = 0;
    check(Some(&current), &next, WriteFlags::default()).unwrap();
}

#[test]
fn modifying_a_carried_extension_gets_the_legacy_error() {
    let current = extended();
    let mut next = extended();
    next.sources
        .push(serde_json::from_value(relay("telegram", "route-telegram")).unwrap());
    assert_eq!(
        check(Some(&current), &next, WriteFlags::default()),
        Err(duplicated(1))
    );
    // The credentials the sources authenticate with are part of the extension.
    let mut rekeyed = extended();
    rekeyed.secrets.channel_key = Some(Secret::new("gate-rotated-key"));
    assert_eq!(
        check(Some(&current), &rekeyed, WriteFlags::default()),
        Err(duplicated(1))
    );
    let mut repartitioned = extended();
    repartitioned.partition.key = cmaeg_core::channel::PartitionKey::Conversation;
    assert_eq!(
        check(Some(&current), &repartitioned, WriteFlags::default()),
        Err(duplicated(1))
    );
}

#[test]
fn removing_the_extension_is_always_allowed() {
    let current = extended();
    let mut next = extended();
    next.sources.truncate(1);
    check(Some(&current), &next, WriteFlags::default()).unwrap();
    // A different single relay is equally legacy-compatible.
    check(Some(&current), &legacy(), WriteFlags::default()).unwrap();
}

#[test]
fn the_dynamic_sources_flag_admits_the_introduction() {
    let flags = WriteFlags {
        dynamic_sources: true,
        ..WriteFlags::default()
    };
    check(None, &extended(), flags).unwrap();
    check(Some(&legacy()), &extended(), flags).unwrap();
    // The routing flag says nothing about sources.
    let routing_only = WriteFlags {
        multi_consumer: true,
        ..WriteFlags::default()
    };
    assert_eq!(check(None, &extended(), routing_only), Err(duplicated(1)));
}

fn with_custom_key(mut definition: ChannelDefinition) -> ChannelDefinition {
    definition.partition.custom_key = Some(cmaeg_core::channel::PartitionKey::EventSubject);
    definition
}

#[test]
fn a_legacy_valid_custom_key_is_refused_as_disabled() {
    let next = with_custom_key(legacy());
    next.validate_legacy().unwrap();
    next.validate().unwrap();
    let disabled = Err(ValidationError::new(
        "/partition/custom_key",
        "dynamic_sources_disabled",
    ));
    assert_eq!(check(None, &next, WriteFlags::default()), disabled);
    assert_eq!(
        check(Some(&legacy()), &next, WriteFlags::default()),
        disabled
    );
    let flags = WriteFlags {
        dynamic_sources: true,
        ..WriteFlags::default()
    };
    check(Some(&legacy()), &next, flags).unwrap();
}

#[test]
fn a_stored_custom_key_is_carried_through_or_removed() {
    let current = with_custom_key(legacy());
    let mut renamed = with_custom_key(legacy());
    renamed.name = "renamed".into();
    check(Some(&current), &renamed, WriteFlags::default()).unwrap();
    check(Some(&current), &legacy(), WriteFlags::default()).unwrap();
    // Changing the custom key itself modifies the extension.
    let mut changed = with_custom_key(legacy());
    changed.partition.custom_key = Some(cmaeg_core::channel::PartitionKey::Fixed);
    assert_eq!(
        check(Some(&current), &changed, WriteFlags::default()),
        Err(ValidationError::new(
            "/partition/custom_key",
            "dynamic_sources_disabled"
        ))
    );
}

use crate::channels::routing_fixture;

/// What a writer behind the multi-consumer flag would have stored.
fn routed() -> ChannelDefinition {
    routing_fixture::definition(routing_fixture::routed())
}

/// The routed channel with neither its policy nor its consumer keys.
fn unrouted() -> ChannelDefinition {
    let mut definition = routed();
    definition.routing = None;
    definition.secrets.consumer_keys.clear();
    definition
}

fn routing_disabled(pointer: &str) -> Result<(), ValidationError> {
    Err(ValidationError::new(pointer, "multi_consumer_disabled"))
}

#[test]
fn introducing_routing_is_refused_until_the_flag_is_on() {
    let next = routed();
    next.validate_legacy().unwrap();
    assert_eq!(
        check(None, &next, WriteFlags::default()),
        routing_disabled("/routing")
    );
    assert_eq!(
        check(Some(&unrouted()), &next, WriteFlags::default()),
        routing_disabled("/routing")
    );
    // The sources flag says nothing about routing.
    let sources_only = WriteFlags {
        dynamic_sources: true,
        ..WriteFlags::default()
    };
    assert_eq!(
        check(Some(&unrouted()), &next, sources_only),
        routing_disabled("/routing")
    );
    let flags = WriteFlags {
        multi_consumer: true,
        ..WriteFlags::default()
    };
    check(None, &next, flags).unwrap();
    check(Some(&unrouted()), &next, flags).unwrap();
}

#[test]
fn consumer_keys_alone_are_a_routing_extension() {
    let mut next = unrouted();
    next.secrets.consumer_keys = routed().secrets.consumer_keys;
    assert_eq!(
        check(Some(&unrouted()), &next, WriteFlags::default()),
        routing_disabled("/secrets/consumer_keys")
    );
}

#[test]
fn an_unchanged_routing_policy_is_carried_through_with_the_flag_off() {
    let current = routed();
    let mut next = routed();
    next.name = "renamed".into();
    next.reply.ack_after_ms = 0;
    check(Some(&current), &next, WriteFlags::default()).unwrap();
}

#[test]
fn modifying_a_carried_routing_policy_is_refused() {
    let current = routed();
    let mut retargeted = routed();
    retargeted.routing.as_mut().unwrap().max_targets = 2;
    let mut relabelled = routed();
    relabelled.routing.as_mut().unwrap().consumers[0].label = Some("Reviewer".into());
    let mut rekeyed = routed();
    rekeyed.secrets.consumer_keys[0].1 = Secret::new("nyx_fixture_rotated");
    for next in [retargeted, relabelled, rekeyed] {
        assert_eq!(
            check(Some(&current), &next, WriteFlags::default()),
            routing_disabled("/routing")
        );
    }
    // Dropping the policy but keeping its keys modifies the extension too.
    let mut keys_only = routed();
    keys_only.routing = None;
    assert_eq!(
        check(Some(&current), &keys_only, WriteFlags::default()),
        routing_disabled("/secrets/consumer_keys")
    );
}

#[test]
fn removing_routing_and_its_keys_is_always_allowed() {
    check(Some(&routed()), &unrouted(), WriteFlags::default()).unwrap();
}

#[test]
fn a_source_refusal_wins_over_a_routing_refusal() {
    let mut next = routed();
    next.sources
        .push(serde_json::from_value(relay("feishu", "route-feishu")).unwrap());
    assert_eq!(
        check(Some(&unrouted()), &next, WriteFlags::default()),
        Err(duplicated(3))
    );
}

#[test]
fn both_extensions_carry_through_together() {
    let mut current = routed();
    current
        .sources
        .push(serde_json::from_value(relay("feishu", "route-feishu")).unwrap());
    let mut next = current.clone();
    next.name = "renamed".into();
    check(Some(&current), &next, WriteFlags::default()).unwrap();
}
