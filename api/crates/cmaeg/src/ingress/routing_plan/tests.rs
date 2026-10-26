use super::*;
use cmaeg_core::{
    activity::RouteResolution,
    channel::{Platform, Secret},
    seal::ReplyAddress,
};
use serde_json::json;

#[path = "fixture.rs"]
mod fixture;
use fixture::*;

#[test]
fn unrouted_legacy_requests_keep_exact_activity_reference_and_default_ids() {
    let mut request = request(
        |v| {
            v.as_object_mut().unwrap().remove("routing");
            v["secrets"]
                .as_object_mut()
                .unwrap()
                .remove("consumer_keys");
        },
        custom(Some("reviewer")),
    );
    // Historical references without the newer activity attestation remain legacy.
    rewrite_ref(&mut request, |event| event.activity_sha256 = None);
    let before = (
        serde_json::to_vec(&request.activity).unwrap(),
        request.event_ref.as_str().to_owned(),
        request.conversation_id.clone(),
        request.idempotency_key.clone(),
    );
    for enabled in [false, true] {
        assert_eq!(run(&request, enabled, false), Ok(OrdinaryPlan::Legacy));
    }
    assert_eq!(
        before,
        (
            serde_json::to_vec(&request.activity).unwrap(),
            request.event_ref.as_str().to_owned(),
            request.conversation_id,
            request.idempotency_key,
        )
    );
}

#[test]
fn routed_default_changes_only_the_attested_activity_digest() {
    let request = request(|_| {}, message("hello"));
    let original = request.event_ref.open(&keys(), NOW).unwrap().payload;
    let planned = routed(&request);
    assert_eq!(planned.targets.len(), 1);
    let target = &planned.targets[0];
    assert!(target.consumer.is_default());
    assert_eq!(target.idempotency_key, request.idempotency_key);
    assert!(target.event.consumer.is_none());
    let mut expected = original;
    expected.activity_sha256 = Some(activity_digest(&planned.activity));
    assert_eq!(target.event, expected);
    assert!(request.activity.routing.is_none());
    assert_eq!(
        planned.activity.routing.as_ref().unwrap().resolution,
        RouteResolution::Default
    );
}

#[test]
fn ordered_targets_share_one_snapshot_and_have_independent_stable_identities() {
    let request = request(targets, message("hello"));
    let planned = routed(&request);
    assert_eq!(
        planned
            .targets
            .iter()
            .map(|target| target.consumer.as_str())
            .collect::<Vec<_>>(),
        [REVIEWER, "default", NYX]
    );
    assert_eq!(
        planned.activity.routing.as_ref().unwrap().targets[0].id,
        id(REVIEWER)
    );
    let original = request.event_ref.open(&keys(), NOW).unwrap().payload;
    for target in &planned.targets {
        assert!(target.event.verifies_activity(&planned.activity));
        assert!(!target.event.verifies_activity(&request.activity));
        assert_eq!(target.event.reply, original.reply);
        assert_eq!(target.event.expires_at, original.expires_at);
        if !target.consumer.is_default() {
            assert_eq!(target.event.consumer.as_ref(), Some(&target.consumer));
            assert_eq!(
                target.event.conversation_id,
                ConversationId::derive_for_consumer(
                    &keys(),
                    &request.channel.id,
                    1,
                    &target.consumer,
                    &original.partition_key
                )
            );
            assert_eq!(
                target.idempotency_key,
                IdempotencyKey::for_consumer(&request.activity.event_id, &target.consumer)
            );
            assert_ne!(target.event.conversation_id, request.conversation_id);
        }
    }
    assert_ne!(
        planned.targets[0].event.conversation_id,
        planned.targets[2].event.conversation_id
    );
    assert_ne!(
        planned.targets[0].idempotency_key,
        planned.targets[2].idempotency_key
    );
    assert_eq!(
        run(&request, true, true).unwrap(),
        OrdinaryPlan::Routed(Box::new(planned))
    );
}

#[test]
fn later_rename_key_rotation_and_policy_change_do_not_mutate_a_plan() {
    let mut request = request(targets, message("hello"));
    let original = routed(&request);
    let definition = &mut std::sync::Arc::get_mut(&mut request.channel)
        .unwrap()
        .definition;
    let policy = definition.routing.as_mut().unwrap();
    policy.consumers[0].name = cmaeg_core::ids::ConsumerName::parse("renamed").unwrap();
    policy.default_targets = vec![id(REVIEWER)];
    definition.secrets.consumer_keys[0].1 = Secret::new("rotated-fixture");
    let later = routed(&request);
    assert_eq!(later.targets.len(), 1);
    assert_eq!(original.targets.len(), 3);
    assert_eq!(
        original.activity.routing.as_ref().unwrap().targets[0]
            .name
            .as_str(),
        "reviewer"
    );
    assert_eq!(
        later.activity.routing.as_ref().unwrap().targets[0]
            .name
            .as_str(),
        "renamed"
    );
    assert_eq!(
        original.targets[0].idempotency_key,
        later.targets[0].idempotency_key
    );
    assert_eq!(
        original.targets[0].event.conversation_id,
        later.targets[0].event.conversation_id
    );
}

#[test]
fn long_partition_is_compact_for_named_targets_without_changing_default() {
    let mut request = request(targets, message("hello"));
    let partition = "partition-".repeat(100);
    request.conversation_id = ConversationId::derive(&keys(), &request.channel.id, 1, &partition);
    let conversation = request.conversation_id.clone();
    rewrite_ref(&mut request, |event| {
        event.partition_key = partition.clone();
        event.conversation_id = conversation;
    });
    let planned = routed(&request);
    assert_eq!(planned.targets[1].event.partition_key, partition);
    assert_eq!(
        planned.targets[1].event.conversation_id,
        request.conversation_id
    );
    assert_eq!(
        planned.targets[0].event.partition_key,
        compact_partition_key(&partition)
    );
    assert_eq!(
        planned.targets[0].event.conversation_id,
        ConversationId::derive_for_consumer(
            &keys(),
            &request.channel.id,
            1,
            &id(REVIEWER),
            &compact_partition_key(&partition)
        )
    );
}

#[test]
fn relay_plans_preserve_exact_reply_authority_for_each_supported_platform() {
    for platform in [
        Platform::Telegram,
        Platform::Lark,
        Platform::Feishu,
        Platform::Discord,
        Platform::Whatsapp,
    ] {
        let mut activity = message("@reviewer hello");
        activity.source = SourceKind::NyxidRelay {
            platform,
            route_id: ROUTE.into(),
        };
        let mut request = request(
            |v| v["sources"][1] = crate::channels::routing_fixture::relay(platform.as_str(), ROUTE),
            activity,
        );
        let reply = ReplyAddress::NyxidRelay {
            platform,
            route_id: ROUTE.into(),
            message_id: "incoming-fixture".into(),
            thread_reply: false,
            reply_token: Secret::new("reply-fixture"),
            reply_token_expires_at: NOW + 100,
        };
        rewrite_ref(&mut request, |event| event.reply = reply.clone());
        let planned = routed(&request);
        assert_eq!(planned.targets.len(), 1);
        assert_eq!(planned.targets[0].consumer, id(REVIEWER));
        assert_eq!(planned.targets[0].event.reply, reply);
        assert_eq!(planned.activity.source, request.activity.source);
    }
}

#[test]
fn signed_event_targets_route_once_and_unsigned_targets_are_refused() {
    let request = request(|_| {}, custom(Some("reviewer,lead,reviewer")));
    let planned = routed(&request);
    assert_eq!(
        planned
            .targets
            .iter()
            .map(|target| target.consumer.as_str())
            .collect::<Vec<_>>(),
        [REVIEWER, "default"]
    );
    assert_eq!(
        planned.activity.routing.as_ref().unwrap().resolution,
        RouteResolution::EventField
    );
    assert_eq!(
        run(&request, true, false),
        Err(PlanError::Routing(RouteError::TargetsUnsigned))
    );
    assert_eq!(
        run(&request, false, false),
        Err(PlanError::Routing(RouteError::KillSwitchClosed))
    );
}

#[test]
fn disabling_routing_uses_only_the_resolvers_safe_legacy_fallback() {
    let request = request(targets, message("hello"));
    assert_eq!(run(&request, false, true), Ok(OrdinaryPlan::Legacy));
    for text in [
        "@reviewer hello",
        "/to reviewer hello",
        "@lead @reviewer hello",
    ] {
        let request = fixture::request(targets, message(text));
        assert_eq!(
            run(&request, false, true),
            Err(PlanError::Routing(RouteError::KillSwitchClosed))
        );
    }
    let request = fixture::request(targets, message("@lead hello"));
    assert_eq!(run(&request, false, true), Ok(OrdinaryPlan::Legacy));
    let request = fixture::request(
        |v| v["routing"]["default_targets"] = json!([REVIEWER]),
        message("hello"),
    );
    assert_eq!(
        run(&request, false, true),
        Err(PlanError::Routing(RouteError::KillSwitchClosed))
    );
}

#[test]
fn non_human_targets_and_missing_credentials_fail_before_producing_launches() {
    let request = request(|_| {}, custom(Some("nyx")));
    assert_eq!(
        run(&request, true, true),
        Err(PlanError::Routing(RouteError::TargetIgnoresActorKind(
            "nyx".into()
        )))
    );
    let mut request = fixture::request(targets, message("hello"));
    std::sync::Arc::get_mut(&mut request.channel)
        .unwrap()
        .definition
        .secrets
        .consumer_keys
        .clear();
    assert_eq!(
        run(&request, true, true),
        Err(PlanError::ConsumerUnavailable)
    );
}

#[test]
fn refuses_modified_activity_and_cross_record_or_conversation_references() {
    for field in ["activity", "record", "conversation", "channel", "digest"] {
        let mut request = request(targets, message("hello"));
        if field == "activity" {
            request.activity.actor.id = "another-human".into();
        } else {
            rewrite_ref(&mut request, |event| match field {
                "record" => {
                    event.record_id = cmaeg_core::ids::RecordId::new("other-record").unwrap()
                }
                "conversation" => {
                    event.conversation_id = ConversationId::new("other-conversation").unwrap()
                }
                "channel" => {
                    event.channel_id =
                        cmaeg_core::ids::ChannelId::new("cmaeg1.ch.other-channel").unwrap()
                }
                _ => event.activity_sha256 = None,
            });
        }
        assert_eq!(
            run(&request, true, true),
            Err(PlanError::ContextMismatch),
            "{field}"
        );
    }
}

#[test]
fn cannot_route_an_expired_reference_or_an_unconfigured_relay_route() {
    let mut request = request(targets, message("hello"));
    rewrite_ref(&mut request, |event| event.expires_at = NOW);
    assert_eq!(run(&request, true, true), Err(PlanError::InvalidReference));
    let mut activity = message("hello");
    activity.source = SourceKind::NyxidRelay {
        platform: Platform::Lark,
        route_id: "other-route".into(),
    };
    let request = fixture::request(targets, activity);
    assert_eq!(run(&request, true, true), Err(PlanError::SourceUnavailable));
}

#[test]
fn cannot_replan_a_named_turn_callback_or_already_routed_activity() {
    let mut request = request(targets, message("hello"));
    let activity = routed(&request).activity;
    request.activity = activity;
    assert_eq!(run(&request, true, true), Err(PlanError::NotOrdinary));
    let mut request = fixture::request(targets, message("hello"));
    request.consumer = Some(id(REVIEWER));
    assert_eq!(run(&request, true, true), Err(PlanError::NotOrdinary));
    let mut request = fixture::request(targets, message("hello"));
    request.activity.kind =
        serde_json::from_value(json!({"type":"interaction","block_id":"choice","action_id":"yes"}))
            .unwrap();
    assert_eq!(run(&request, true, true), Err(PlanError::NotOrdinary));
    let mut request = fixture::request(targets, message("hello"));
    rewrite_ref(&mut request, |event| event.consumer = Some(id(REVIEWER)));
    assert_eq!(run(&request, true, true), Err(PlanError::NotOrdinary));
}

#[test]
fn long_event_ids_with_the_same_default_key_remain_distinct_for_named_targets() {
    let make = |suffix: &str| {
        let mut activity = message("@reviewer hello");
        activity.event_id = event_id(&format!("{}-{suffix}", "a".repeat(210)));
        request(|_| {}, activity)
    };
    let first = make("first");
    let second = make("second");
    assert_eq!(first.idempotency_key, second.idempotency_key);
    assert_ne!(
        routed(&first).targets[0].idempotency_key,
        routed(&second).targets[0].idempotency_key
    );
}
