use super::{
    journal_fixture::{definition, Fixture},
    normalize::{normalize, Normalized},
    Dispatch, GatewayError, InboxLaunch, InboxReader,
};
use crate::{
    channels::resolver::ResolvedChannel, connections::store::index_id, ingress::admission::Refusal,
};
use cmaeg_core::{
    activity::{ActivityKind, ConversationKind, SourceKind},
    channel::{AdmissionPolicy, Secret},
    seal::{ChannelPointer, ReplyAddress},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Instant};

pub(super) fn raw() -> Value {
    json!({"id":"700","channel_id":"99","author":{"id":"42","bot":false,"username":"human"},
        "type":0,"timestamp":"2026-10-26T00:00:00Z","content":"hello","attachments":[],"mentions":[]})
}
pub(super) fn channel() -> Arc<ResolvedChannel> {
    let definition = definition();
    Arc::new(ResolvedChannel {
        id: "cmaeg1.ch.discord-fixture".parse().unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://records.example/channel".into(),
        },
        definition,
        agent_key_hash: [7; 32],
        consumer_keys: vec![],
        loaded_at: Instant::now(),
        version: 1,
    })
}
#[test]
fn native_message_identity_dm_and_group_admission_are_platform_facts() {
    let Normalized::Activity(dm) = normalize(&raw(), "77", &AdmissionPolicy::All {}).unwrap()
    else {
        panic!()
    };
    assert_eq!(dm.event_id.as_str(), "discord:77:700");
    assert_eq!(
        dm.source,
        SourceKind::Discord {
            bot_id: "77".into()
        }
    );
    assert_eq!(dm.conversation.kind, ConversationKind::Private);
    let policy: AdmissionPolicy = serde_json::from_value(
        json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention_or_reply_to_bot"}),
    )
    .unwrap();
    let mut group = raw();
    group["guild_id"] = "88".into();
    assert!(matches!(
        normalize(&group, "77", &policy).unwrap(),
        Normalized::Refused(Refusal::GroupNotAddressed)
    ));
    group["mentions"] = json!([{"id":"77"}]);
    assert!(matches!(
        normalize(&group, "77", &policy).unwrap(),
        Normalized::Activity(_)
    ));
}
#[test]
fn native_bot_webhook_system_edit_and_malformed_events_do_not_launch() {
    for (key, value, reason) in [
        ("webhook_id", json!("900"), Refusal::BotSender),
        (
            "edited_timestamp",
            json!("2026-10-26T00:01:00Z"),
            Refusal::MessageEdit,
        ),
        ("type", json!(7), Refusal::SystemEvent),
    ] {
        let mut event = raw();
        event[key] = value;
        assert!(
            matches!(normalize(&event,"77",&AdmissionPolicy::All{}).unwrap(),Normalized::Refused(actual) if actual==reason)
        );
    }
    for value in [json!("yes"), json!(1), Value::Null] {
        let mut event = raw();
        event["author"]["bot"] = value;
        assert!(matches!(
            normalize(&event, "77", &AdmissionPolicy::All {}),
            Err(GatewayError::Protocol)
        ));
    }
    let mut event = raw();
    event["id"] = "007".into();
    assert!(normalize(&event, "77", &AdmissionPolicy::All {}).is_err());
    event = raw();
    event["message_reference"] = json!({"message_id":"600","channel_id":"98"});
    event["referenced_message"] = json!({"id":"600","channel_id":"98","author":{"id":"77"}});
    assert!(normalize(&event, "77", &AdmissionPolicy::All {}).is_err());
}
#[test]
fn every_native_attachment_has_exact_nonsecret_locator_and_malformed_entries_are_not_dropped() {
    let mut event = raw();
    event["content"] = "".into();
    event["attachments"] = json!([{"id":"500","filename":"photo.png","content_type":"image/png","size":4,"url":"https://cdn.discordapp.com/attachments/99/500/photo.png?hm=secret"}]);
    let Normalized::Activity(activity) = normalize(&event, "77", &AdmissionPolicy::All {}).unwrap()
    else {
        panic!()
    };
    let ActivityKind::Message { attachments, .. } = &activity.kind else {
        panic!()
    };
    assert_eq!(attachments.len(), 1);
    let public = serde_json::to_string(&activity).unwrap();
    assert!(public.contains("attachment_id"));
    assert!(!public.contains("hm=secret"));
    event["attachments"][0]["id"] = Value::Null;
    assert!(normalize(&event, "77", &AdmissionPolicy::All {}).is_err());
}
#[tokio::test]
async fn durable_inbox_restores_original_snapshot_and_stable_activity_across_replay() {
    let f = Fixture::new().await;
    let journal = f.acquire().await.unwrap();
    let mut event = raw();
    event["id"] = f.message_id().into();
    let dispatch = Dispatch {
        name: "MESSAGE_CREATE".into(),
        sequence: 1,
        data: event,
    };
    journal
        .persist_message(&journal.authority, &dispatch)
        .await
        .unwrap();
    let id = index_id(
        "discord-native-message-v1",
        &[&f.record, "77", dispatch.data["id"].as_str().unwrap()],
    );
    let reader = InboxReader::new(f.store.channels.clone(), f.store.keys.clone());
    let now = (f.store.clock)() / 1000;
    let InboxLaunch::Launch(first) = reader.load(&id, channel(), now).await.unwrap() else {
        panic!()
    };
    assert_eq!(first.received_at, now);
    let stored: Value = serde_json::from_slice(&first.source_bytes).unwrap();
    assert_eq!(stored["dispatch"], dispatch.data);
    assert_eq!(stored["channel"], channel().definition.to_public_json());
    let opened = first
        .request
        .event_ref
        .open(&f.store.keys, now)
        .unwrap()
        .payload;
    assert!(opened.verifies_activity(&first.request.activity));
    assert_eq!(opened.expires_at, now + 1800);
    assert!(
        matches!(opened.reply,ReplyAddress::Discord{bot_id,channel_id,..} if bot_id=="77" && channel_id=="99")
    );
    let mut changed = channel();
    Arc::get_mut(&mut changed).unwrap().definition.partition.key =
        cmaeg_core::channel::PartitionKey::Conversation;
    f.advance(60_000);
    let mut redelivery = dispatch;
    redelivery.data["content"] = "replacement must not change stored bytes".into();
    journal
        .persist_message(&journal.authority, &redelivery)
        .await
        .unwrap();
    let InboxLaunch::Launch(second) = reader.load(&id, changed, now + 60).await.unwrap() else {
        panic!()
    };
    assert_eq!(first.source_bytes, second.source_bytes);
    assert_eq!(first.received_at, second.received_at);
    assert_eq!(first.request.activity, second.request.activity);
    assert_eq!(
        first.request.conversation_id,
        second.request.conversation_id
    );
    assert_eq!(
        first.request.idempotency_key,
        second.request.idempotency_key
    );
    assert_eq!(
        second
            .request
            .event_ref
            .open(&f.store.keys, now + 60)
            .unwrap()
            .payload
            .expires_at,
        now + 1800
    );
    assert!(matches!(
        reader.load(&id, channel(), now + 1800).await,
        Err(GatewayError::Authority)
    ));
    // Expired reply authority does not erase the retained record for inspection.
    assert!(f.store.load::<Value>(&id, true).await.unwrap().is_some());
    let mut rotated = channel();
    Arc::get_mut(&mut rotated)
        .unwrap()
        .definition
        .secrets
        .discord_bot_token = Some(Secret::new("changed-native-fixture"));
    assert!(matches!(
        reader.load(&id, rotated, now).await,
        Err(GatewayError::Authority)
    ));
    assert!(reader.load(&id, channel(), now + 8 * 86400).await.is_err());
}

#[tokio::test]
async fn inbox_without_original_receipt_time_is_never_granted_new_authority() {
    let f = Fixture::new().await;
    let journal = f.acquire().await.unwrap();
    let mut event = raw();
    event["id"] = f.message_id().into();
    let dispatch = Dispatch {
        name: "MESSAGE_CREATE".into(),
        sequence: 1,
        data: event,
    };
    journal
        .persist_message(&journal.authority, &dispatch)
        .await
        .unwrap();
    let id = index_id(
        "discord-native-message-v1",
        &[&f.record, "77", dispatch.message_id().unwrap()],
    );
    let (mut legacy, _) = f.store.load::<Value>(&id, true).await.unwrap().unwrap();
    legacy.as_object_mut().unwrap().remove("received_at");
    f.memory.rows.lock().unwrap().get_mut(&id).unwrap().blob = cmaeg_core::seal::seal_interaction(
        &f.store.keys,
        &id,
        &serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    let reader = InboxReader::new(f.store.channels.clone(), f.store.keys.clone());
    assert!(matches!(
        reader.load(&id, channel(), (f.store.clock)() / 1000).await,
        Err(GatewayError::Protocol)
    ));
    assert_eq!(
        journal.persist_message(&journal.authority, &dispatch).await,
        Err(GatewayError::Protocol)
    );
    assert!(f.store.load::<Value>(&id, true).await.unwrap().is_some());
}

#[tokio::test]
async fn native_inbox_refuses_thread_partition_without_proven_thread_metadata() {
    let f = Fixture::new().await;
    let mut original = definition();
    original.partition.key = cmaeg_core::channel::PartitionKey::Thread;
    f.replace(&original);
    let journal = f.acquire().await.unwrap();
    let mut event = raw();
    event["id"] = f.message_id().into();
    let dispatch = Dispatch {
        name: "MESSAGE_CREATE".into(),
        sequence: 1,
        data: event,
    };
    journal
        .persist_message(&journal.authority, &dispatch)
        .await
        .unwrap();
    let id = index_id(
        "discord-native-message-v1",
        &[&f.record, "77", dispatch.message_id().unwrap()],
    );
    let reader = InboxReader::new(f.store.channels.clone(), f.store.keys.clone());
    assert!(matches!(
        reader
            .load(&id, channel(), (f.store.clock)() / 1000)
            .await
            .unwrap(),
        InboxLaunch::Refused(Refusal::ThreadEvidenceUnavailable)
    ));
}

#[test]
fn native_mention_only_and_reply_only_policies_use_their_exact_evidence() {
    let mut message = raw();
    message["guild_id"] = "88".into();
    message["mentions"] = json!([{"id":"77"}]);
    let policy = |groups| {
        serde_json::from_value(json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":groups})).unwrap()
    };
    assert!(matches!(
        normalize(&message, "77", &policy("reply_to_bot")).unwrap(),
        Normalized::Refused(Refusal::GroupNotAddressed)
    ));
    message["mentions"] = json!([]);
    message["message_reference"] = json!({"message_id":"600","channel_id":"99"});
    message["referenced_message"] = json!({"id":"600","channel_id":"99","author":{"id":"77"}});
    assert!(matches!(
        normalize(&message, "77", &policy("mention")).unwrap(),
        Normalized::Refused(Refusal::GroupNotAddressed)
    ));
    assert!(matches!(
        normalize(&message, "77", &policy("reply_to_bot")).unwrap(),
        Normalized::Activity(_)
    ));
}

#[test]
fn scoped_discord_admission_matches_exact_sender_and_chat_ids() {
    let policy: AdmissionPolicy = serde_json::from_value(json!({
        "type":"scoped",
        "senders":{"type":"allowlist","ids":["42"]},
        "chats":{"type":"allowlist","ids":["99"]},
        "groups":"all"
    }))
    .unwrap();
    for (sender, chat, expected) in [
        ("42", "99", None),
        ("420", "99", Some(Refusal::SenderDenied)),
        ("42", "990", Some(Refusal::ChatDenied)),
        ("420", "990", Some(Refusal::SenderDenied)),
    ] {
        for kind in ["private", "group", "channel"] {
            let mut event = raw();
            event["author"]["id"] = sender.into();
            event["channel_id"] = chat.into();
            // Names are presentation only; the decision uses platform IDs.
            event["author"]["username"] = "renamed".into();
            match kind {
                "group" => event["channel_type"] = 3.into(),
                "channel" => event["guild_id"] = "88".into(),
                _ => {}
            }
            match (normalize(&event, "77", &policy).unwrap(), expected) {
                (Normalized::Activity(activity), None) => {
                    assert_eq!(activity.conversation.id, chat);
                    assert_eq!(activity.actor.id, sender);
                }
                (Normalized::Refused(actual), Some(expected)) => assert_eq!(actual, expected),
                _ => panic!("unexpected admission for {kind}"),
            }
        }
    }
}

#[test]
fn scoped_discord_admission_supports_independent_dimensions_and_explicit_deny() {
    let open = json!({"type":"open"});
    let deny = json!({"type":"deny"});
    let sender = json!({"type":"allowlist","ids":["42"]});
    let chat = json!({"type":"allowlist","ids":["99"]});
    for (senders, chats, expected) in [
        (sender.clone(), open.clone(), None),
        (open.clone(), chat.clone(), None),
        (deny.clone(), chat, Some(Refusal::SenderDenied)),
        (sender, deny, Some(Refusal::ChatDenied)),
        (open.clone(), open, None),
    ] {
        let policy = serde_json::from_value(json!({
            "type":"scoped","senders":senders,"chats":chats,"groups":"all"
        }))
        .unwrap();
        match (normalize(&raw(), "77", &policy).unwrap(), expected) {
            (Normalized::Activity(_), None) => {}
            (Normalized::Refused(actual), Some(expected)) => assert_eq!(actual, expected),
            _ => panic!("unexpected scoped admission"),
        }
    }
}
