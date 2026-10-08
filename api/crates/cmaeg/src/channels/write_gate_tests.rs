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
