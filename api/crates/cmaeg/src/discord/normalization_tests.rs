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
    let opened = first.event_ref.open(&f.store.keys, now).unwrap().payload;
    assert!(opened.verifies_activity(&first.activity));
    assert!(
        matches!(opened.reply,ReplyAddress::Discord{bot_id,channel_id,..} if bot_id=="77" && channel_id=="99")
    );
    let mut changed = channel();
    Arc::get_mut(&mut changed).unwrap().definition.partition.key =
        cmaeg_core::channel::PartitionKey::Conversation;
    let InboxLaunch::Launch(second) = reader.load(&id, changed, now).await.unwrap() else {
        panic!()
    };
    assert_eq!(first.activity, second.activity);
    assert_eq!(first.conversation_id, second.conversation_id);
    assert_eq!(first.idempotency_key, second.idempotency_key);
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
