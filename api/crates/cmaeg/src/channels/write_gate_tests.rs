//! The gate's rules on constructed definitions. No storage is involved: the
//! carry-through and removal rules are decided from the two definitions alone.
use super::{check, WriteFlags};
use cmaeg_core::{
    channel::{ChannelDefinition, Extensions, Secret},
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

#[test]
fn native_bot_mentions_require_writes_flag_and_can_be_carried_or_removed() {
    let base = legacy();
    let mut value = serde_json::to_value(&base).unwrap();
    value["sources"][0]["admission"] = serde_json::json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"all","bot_messages":"native_mentions"});
    value["sources"][0]["bot_id"] = serde_json::json!("ou_receiver");
    let opted: ChannelDefinition = serde_json::from_value(value).unwrap();
    assert!(check(None, &opted, WriteFlags::default()).is_err());
    check(
        None,
        &opted,
        WriteFlags {
            native_bot_mentions: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut renamed = opted.clone();
    renamed.name = "renamed".into();
    check(Some(&opted), &renamed, WriteFlags::default()).unwrap();
    check(Some(&opted), &base, WriteFlags::default()).unwrap();
    assert!(!Extensions::of(&opted).legacy_compatible());
}

#[test]
fn native_threads_require_writes_flag_but_retained_contracts_and_removal_keep_working() {
    let mut value = serde_json::to_value(legacy()).unwrap();
    value["secrets"]["agent_key_hash"] =
        crate::channels::definition::hash_secret("thread-policy-fixture").into();
    value["sources"][0]["bot_id"] = json!("ou_receiver");
    let base: ChannelDefinition = serde_json::from_value(value.clone()).unwrap();
    value["reply"]["thread_contract"] = json!({"version":1,"follow_chat_ids":["oc_follow"]});
    let opted: ChannelDefinition = serde_json::from_value(value).unwrap();
    opted.validate().unwrap();
    let refused = Err(ValidationError::new(
        "/reply/thread_contract",
        "thread_contract_writes_disabled",
    ));
    assert_eq!(check(None, &opted, WriteFlags::default()), refused);
    assert_eq!(check(Some(&base), &opted, WriteFlags::default()), refused);
    check(
        None,
        &opted,
        WriteFlags {
            thread_contract: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut renamed = opted.clone();
    renamed.name = "renamed".into();
    check(Some(&opted), &renamed, WriteFlags::default()).unwrap();
    check(Some(&opted), &base, WriteFlags::default()).unwrap();
    let mut changed = opted.clone();
    changed
        .reply
        .thread_contract
        .as_mut()
        .unwrap()
        .follow_chat_ids
        .push("oc_new".into());
    assert_eq!(
        check(Some(&opted), &changed, WriteFlags::default()),
        refused
    );
    assert!(!Extensions::of(&opted).legacy_compatible());
}

#[test]
fn verified_relay_mode_requires_its_own_writes_and_retains_exact_existing_sources() {
    for platform in ["discord", "whatsapp"] {
        let mut raw = relay(platform, "retained-route");
        raw["verify_raw_event"] = true.into();
        let opted = definition(json!([raw]));
        opted.validate().unwrap();
        opted.validate_legacy_sources_and_partition().unwrap();
        assert!(Extensions::of(&opted).sources);
        assert!(!Extensions::of(&opted).legacy_compatible());
        let refused = Err(ValidationError::new(
            "/sources/0/verify_raw_event",
            "relay_raw_verification_writes_disabled",
        ));
        assert_eq!(check(None, &opted, WriteFlags::default()), refused);
        assert_eq!(
            check(Some(&legacy()), &opted, WriteFlags::default()),
            refused
        );
        assert_eq!(
            check(
                None,
                &opted,
                WriteFlags {
                    multi_consumer: true,
                    ..Default::default()
                }
            ),
            refused
        );
        check(
            None,
            &opted,
            WriteFlags {
                relay_raw_verification: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut renamed = opted.clone();
        renamed.name = "renamed without changing verification".into();
        check(Some(&opted), &renamed, WriteFlags::default()).unwrap();
        let mut changed = opted.clone();
        if let cmaeg_core::channel::Source::NyxidRelay { route_ids, .. } = &mut changed.sources[0] {
            route_ids.push("new-route".into());
        }
        assert_eq!(
            check(Some(&opted), &changed, WriteFlags::default()),
            refused
        );
        let mut rotated = opted.clone();
        rotated.secrets.channel_key = Some(Secret::new("rotated-test-credential"));
        assert_eq!(
            check(Some(&opted), &rotated, WriteFlags::default()),
            refused
        );
        // An explicit owner-selected return to a retained legacy policy is
        // allowed; the automatic rollback audit never makes this decision.
        let mut legacy_source = relay(platform, "retained-route");
        legacy_source["trust_normalized"] = true.into();
        let restored = definition(json!([legacy_source]));
        assert!(Extensions::of(&restored).legacy_compatible());
        check(Some(&opted), &restored, WriteFlags::default()).unwrap();
    }
}

#[test]
fn lark_resources_require_own_gate_and_carry_or_remove_without_reauthoring() {
    let base = legacy();
    let mut raw = base.to_blob_json();
    raw["sources"][0]["bot_id"] = json!("ou_receiver");
    raw["sources"][0]["lark_resources"] = json!({"user_service_id":"connection-one","catalog_service_id":"catalog-one","app_id":"cli_source"});
    let opted: ChannelDefinition = serde_json::from_value(raw).unwrap();
    opted.validate().unwrap();
    let disabled = Err(ValidationError::new(
        "/sources/0/lark_resources",
        "lark_resource_writes_disabled",
    ));
    assert_eq!(check(None, &opted, WriteFlags::default()), disabled);
    assert_eq!(
        check(
            Some(&base),
            &opted,
            WriteFlags {
                dynamic_sources: true,
                multi_consumer: true,
                ..Default::default()
            }
        ),
        disabled
    );
    check(
        None,
        &opted,
        WriteFlags {
            lark_resources: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut renamed = opted.clone();
    renamed.name = "Unrelated edit".into();
    check(Some(&opted), &renamed, WriteFlags::default()).unwrap();
    check(Some(&opted), &base, WriteFlags::default()).unwrap();
    let mut changed = opted.clone();
    if let cmaeg_core::channel::Source::NyxidRelay {
        lark_resources: Some(binding),
        ..
    } = &mut changed.sources[0]
    {
        binding.user_service_id = "connection-two".into();
    }
    assert_eq!(
        check(Some(&opted), &changed, WriteFlags::default()),
        disabled
    );
    assert!(!Extensions::of(&opted).legacy_compatible());
}

#[test]
fn lark_binding_requires_independent_bot_pin_regional_raw_source_and_credential() {
    let mut raw = legacy().to_blob_json();
    raw["sources"][0]["bot_id"] = json!("ou_receiver");
    raw["sources"][0]["lark_resources"] = json!({"user_service_id":"connection-one","catalog_service_id":"catalog-one","app_id":"cli_source"});
    for (pointer, value) in [
        ("/sources/0/bot_id", Value::Null),
        ("/sources/0/bot_id", json!("cli_wrong_kind")),
        ("/sources/0/platform", json!("telegram")),
        ("/sources/0/trust_normalized", json!(true)),
        ("/sources/0/lark_resources/app_id", json!("ou_wrong_kind")),
        (
            "/sources/0/lark_resources/user_service_id",
            json!("../other"),
        ),
        (
            "/sources/0/lark_resources/catalog_service_id",
            json!("other?connection=1"),
        ),
        ("/secrets/channel_key", Value::Null),
    ] {
        let mut invalid = raw.clone();
        if pointer == "/sources/0/trust_normalized" {
            invalid["sources"][0]["trust_normalized"] = value;
        } else {
            *invalid.pointer_mut(pointer).unwrap() = value;
        }
        let definition: ChannelDefinition = serde_json::from_value(invalid).unwrap();
        assert!(definition.validate().is_err(), "{pointer}");
    }
    // Existing sources still omit the opt-in rather than publishing a new null property.
    assert!(serde_json::to_value(legacy()).unwrap()["sources"][0]
        .get("lark_resources")
        .is_none());
}

#[test]
fn native_thread_history_requires_its_gate_but_retention_and_removal_are_allowed() {
    let mut next = definition(
        json!([{"type":"discord","bot_id":"77","intents":37377,"admission":{"type":"all"},"thread_history":true}]),
    );
    next.secrets.discord_bot_token = Some(Secret::new("history-gate-fixture"));
    let flags = WriteFlags {
        native_discord: true,
        ..Default::default()
    };
    let error = check(None, &next, flags).unwrap_err();
    assert_eq!(
        error,
        ValidationError::new(
            "/sources/0/thread_history",
            "discord_thread_history_disabled"
        )
    );
    check(
        None,
        &next,
        WriteFlags {
            discord_thread_history: true,
            ..flags
        },
    )
    .unwrap();
    let mut renamed = next.clone();
    renamed.name = "rename".into();
    check(Some(&next), &renamed, flags).unwrap();
    let cmaeg_core::channel::Source::Discord { thread_history, .. } = &mut renamed.sources[0]
    else {
        panic!()
    };
    *thread_history = false;
    check(Some(&next), &renamed, flags).unwrap();
    assert!(serde_json::to_value(&renamed.sources[0])
        .unwrap()
        .get("thread_history")
        .is_none());
}

#[path = "write_gate_whatsapp_groups_tests.rs"]
mod whatsapp_groups;
