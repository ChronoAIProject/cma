use crate::channels::{resolver::ResolvedChannel, routing_fixture};
use crate::ingress::{
    launch::LaunchRequest,
    routing_plan::{plan, OrdinaryPlan, PlanError, RoutedPlan, RoutingOptions},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    ids::{ChannelId, ConsumerId, ConversationId, EventId, IdempotencyKey},
    seal::{
        event_ref::activity_digest, ChannelPointer, EventRef, EventRefPayload, MasterKeyRing,
        ReplyAddress,
    },
};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc, time::Instant};

pub const NOW: i64 = 1_800_000_000;
pub use routing_fixture::{NYX, REVIEWER, ROUTE};

pub fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([7; 32]))).unwrap()
}

pub fn message(text: &str) -> Activity {
    serde_json::from_value(json!({
        "schema_version":1,"event_id":"event-1",
        "kind":{"type":"message","text":text,"attachments":[],"mentions_bot":false},
        "actor":{"id":"human-1","kind":"human"},
        "conversation":{"id":"42","kind":"private"},
        "occurred_at":"2026-10-26T00:00:00Z",
        "source":{"type":"telegram","platform":"telegram","bot_id":"77"}
    }))
    .unwrap()
}

pub fn custom(targets: Option<&str>) -> Activity {
    let mut activity = message("");
    let mut ce = json!({"specversion":"1.0","id":"event-1",
        "source":"https://producer.example/build","type":"build.done"});
    if let Some(targets) = targets {
        ce["cmaegtargets"] = targets.into();
    }
    activity.kind = serde_json::from_value(json!({"type":"event","ce":ce})).unwrap();
    activity.source = SourceKind::Custom;
    activity.actor.kind = cmaeg_core::activity::ActorKind::System;
    activity.conversation.kind = cmaeg_core::activity::ConversationKind::Custom;
    activity
}

pub fn request(edit: impl FnOnce(&mut Value), activity: Activity) -> LaunchRequest {
    let mut value = routing_fixture::routed();
    edit(&mut value);
    let definition = routing_fixture::definition(value);
    definition.validate().unwrap();
    let channel = Arc::new(ResolvedChannel {
        id: ChannelId::new("cmaeg1.ch.planner-fixture").unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/records/fixture".into(),
        },
        definition,
        agent_key_hash: [7; 32],
        consumer_keys: Vec::new(),
        loaded_at: Instant::now(),
        version: 1,
    });
    let partition_key = activity.partition_key(&channel.definition.partition);
    let conversation_id = ConversationId::derive(&keys(), &channel.id, 1, &partition_key);
    let payload = EventRefPayload {
        turn_trace: None,
        v: 1,
        activity_sha256: Some(activity_digest(&activity)),
        channel_id: channel.id.clone(),
        record_id: channel.pointer.record_id.clone(),
        event_id: activity.event_id.clone(),
        partition_key,
        conversation_id: conversation_id.clone(),
        expires_at: NOW + 900,
        reply: match &activity.source {
            SourceKind::Telegram { bot_id, .. } => ReplyAddress::Telegram {
                bot_id: bot_id.clone(),
                chat_id: activity.conversation.id.clone(),
                message_id: "1".into(),
                thread_id: None,
            },
            SourceKind::NyxidRelay { platform, route_id } => ReplyAddress::NyxidRelay {
                platform: *platform,
                route_id: route_id.clone(),
                message_id: "incoming-fixture".into(),
                thread_reply: false,
                reply_token: cmaeg_core::channel::Secret::new("reply-fixture"),
                reply_token_expires_at: NOW + 100,
            },
            SourceKind::NativeLark(_)
            | SourceKind::Custom
            | SourceKind::Discord { .. }
            | SourceKind::Whatsapp { .. }
            | SourceKind::Github { .. } => ReplyAddress::None,
        },
        consumer: None,
        lineage: None,
        telegram_admission: None,
    };
    payload.validate().unwrap();
    LaunchRequest {
        channel,
        idempotency_key: IdempotencyKey::from_event(&activity.event_id),
        activity,
        conversation_id,
        event_ref: EventRef::seal(&payload, &keys()).unwrap(),
        received_at: Instant::now(),
        trace: tracing::Span::none(),
        timing: None,
        turn_trace: None,
        consumer: None,
    }
}

pub fn rewrite_ref(request: &mut LaunchRequest, edit: impl FnOnce(&mut EventRefPayload)) {
    let mut event = request.event_ref.open(&keys(), NOW).unwrap().payload;
    edit(&mut event);
    request.event_ref = EventRef::seal(&event, &keys()).unwrap();
}

pub fn run(
    request: &LaunchRequest,
    enabled: bool,
    signed: bool,
) -> Result<OrdinaryPlan, PlanError> {
    plan(
        request,
        &keys(),
        NOW,
        RoutingOptions {
            enabled,
            attributes_signed: signed,
            non_human_providers: &BTreeSet::from(["cma_codex".into()]),
        },
    )
}

pub fn routed(request: &LaunchRequest) -> RoutedPlan {
    match run(request, true, true).unwrap() {
        OrdinaryPlan::Routed(plan) => *plan,
        OrdinaryPlan::Legacy => panic!("expected routed plan"),
    }
}

pub fn targets(value: &mut Value) {
    value["routing"]["default_targets"] = json!([REVIEWER, "default", NYX]);
}

pub fn id(value: &str) -> ConsumerId {
    ConsumerId::parse(value).unwrap()
}

pub fn event_id(value: &str) -> EventId {
    EventId::new(value).unwrap()
}
