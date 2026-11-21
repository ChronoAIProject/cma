use super::{
    dispatch_fixture::Setup, inbox_reader::Body, inbox_scan::InboxStore, journal_model::Inbox,
    message_event::normalize_change, Dispatch,
};
use crate::{connections::store::index_id, ingress::routing_store::AdmittedPlan};
use cmaeg_core::activity::{ActivityKind, ActorKind};
use serde_json::{json, Value};
use std::sync::Arc;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub(super) fn edit(f: &Setup) -> Dispatch {
    Dispatch {
        name: "MESSAGE_UPDATE".into(),
        sequence: 2,
        data: json!({"id":f.fixture.message_id(),"channel_id":"99","content":"edited instruction",
            "edited_timestamp":OffsetDateTime::from_unix_timestamp(f.now()).unwrap().format(&Rfc3339).unwrap()}),
    }
}
pub(super) fn delete(f: &Setup) -> Dispatch {
    Dispatch {
        name: "MESSAGE_DELETE".into(),
        sequence: 3,
        data: json!({"id":f.fixture.message_id(),"channel_id":"99"}),
    }
}
pub(super) async fn persist(f: &Setup, event: &Dispatch) {
    f.journal
        .persist_message_event(&f.journal.authority, event)
        .await
        .unwrap();
}
async fn plans(f: &Setup) -> Vec<AdmittedPlan> {
    f.plans.recover_all_page(None, 32).await.unwrap().plans
}

#[tokio::test]
async fn discord_message_edits_and_unknown_actor_deletion_keep_original_conversation() {
    let mut f = Setup::new(true).await;
    f.channel.partition.key = cmaeg_core::channel::PartitionKey::ConversationAndSender;
    f.fixture.replace(&f.channel);
    Arc::get_mut(&mut f.journal)
        .unwrap()
        .authority
        .public_definition["partition"]["key"] = "conversation_and_sender".into();
    f.message(|_| {}).await;
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    let original = plans(&f).await.remove(0);
    persist(&f, &edit(&f)).await;
    persist(&f, &delete(&f)).await;
    let page = f.sweep(f.now()).await;
    assert_eq!((page.acknowledged, page.pending, page.invalid), (3, 0, 0));
    let plans = plans(&f).await;
    assert_eq!(plans.len(), 3);
    assert_eq!(
        plans
            .iter()
            .filter(|p| matches!(p.activity.kind, ActivityKind::Message { .. }))
            .count(),
        1
    );
    for plan in plans.iter().filter(|p| p.identity != original.identity) {
        assert_eq!(
            plan.targets[0].launch.event.conversation_id,
            original.targets[0].launch.event.conversation_id
        );
        assert_eq!(
            plan.targets[0].launch.event.partition_key,
            original.targets[0].launch.event.partition_key
        );
        let ActivityKind::Event { ce, .. } = &plan.activity.kind else {
            panic!()
        };
        assert_eq!(
            ce.data.as_ref().unwrap()["original_event_id"],
            original.identity.event_id.as_str()
        );
        assert_eq!(ce.data.as_ref().unwrap()["original_author"]["id"], "42");
        if ce.r#type == "cma.message.deleted.v1" {
            assert_eq!(plan.activity.actor.kind, ActorKind::Unknown);
            assert_eq!(plan.activity.actor.id, "unknown");
            assert!(ce.time.is_none());
        } else {
            assert_eq!(plan.activity.actor.kind, ActorKind::Human);
            assert_eq!(
                ce.data.as_ref().unwrap()["patch"]["text"],
                "edited instruction"
            );
        }
    }
    assert_eq!(f.direct_calls(), 0);
}

#[tokio::test]
async fn discord_change_replay_lost_manifest_and_ack_keep_one_original_snapshot() {
    for after_commit in [true, false] {
        let f = Setup::new(true).await;
        f.message(|_| {}).await;
        f.sweep(f.now()).await;
        let event = edit(&f);
        let event_id = format!(
            "discord:77:{}:edit:{}",
            f.fixture.message_id(),
            i128::from(f.now()) * 1_000_000_000
        );
        let id = index_id(
            "discord-native-message-event-v1",
            &[&f.fixture.record, "77", &event_id],
        );
        f.fixture.fail(&id, after_commit);
        let first = f
            .journal
            .persist_message_event(&f.journal.authority, &event)
            .await;
        assert_eq!(first.is_ok(), after_commit);
        persist(&f, &event).await;
        let store = InboxStore::new(
            f.fixture.store.channels.clone(),
            f.fixture.store.keys.clone(),
        );
        let page = store.page(None, f.now()).await.unwrap();
        let entry = page.entries.iter().find(|entry| entry.id == id).unwrap();
        f.fixture.fail(&entry.ack_id(), false);
        assert_eq!(f.sweep(f.now()).await.pending, 1);
        let first = plans(&f)
            .await
            .into_iter()
            .find(|p| p.identity.event_id.as_str() == event_id)
            .unwrap();
        let mut changed = Dispatch {
            name: event.name.clone(),
            sequence: event.sequence,
            data: event.data.clone(),
        };
        changed.sequence += 100;
        changed.data["content"] = "conflicting same-revision retry".into();
        persist(&f, &changed).await;
        assert_eq!(f.sweep(f.now()).await.pending, 0);
        let next = plans(&f).await;
        assert_eq!(next.len(), 2);
        assert_eq!(
            next.into_iter()
                .find(|p| p.identity == first.identity)
                .unwrap(),
            first
        );
        assert_eq!(f.direct_calls(), 0);
    }
}

#[tokio::test]
async fn discord_message_change_waits_for_original_admission_without_ordinary_fallback() {
    let f = Setup::new(true).await;
    f.message(|_| {}).await;
    persist(&f, &edit(&f)).await;
    // Iterate safely regardless of lexicographic inbox order: the event either
    // waits for original admission or loads its already committed target.
    f.sweep(f.now()).await;
    assert_eq!(f.sweep(f.now()).await.pending, 0);
    assert_eq!(plans(&f).await.len(), 2);
    assert_eq!(f.direct_calls(), 0);
}

#[tokio::test]
async fn discord_unknown_mismatched_refused_and_malformed_changes_never_launch() {
    let f = Setup::new(true).await;
    f.message(|_| {}).await;
    f.sweep(f.now()).await;
    for defect in 0..9 {
        let mut event = edit(&f);
        match defect {
            0 => event.data["id"] = "700".into(),
            1 => event.data["channel_id"] = "100".into(),
            2 => event.data["guild_id"] = "88".into(),
            3 => event.data["author"] = json!({"id":"43"}),
            4 => event.data["author"] = json!({"id":"42","bot":true}),
            5 => event.data["edited_timestamp"] = Value::Null,
            6 => event.data["content"] = json!(12),
            7 => {
                event.data.as_object_mut().unwrap().remove("content");
                event.data["embeds"] = json!([]);
            }
            8 => event.data["attachments"] = json!([{"id":"invalid"}]),
            _ => unreachable!(),
        }
        persist(&f, &event).await;
    }
    f.sweep(f.now()).await;
    assert_eq!(plans(&f).await.len(), 1);
    assert_eq!(f.direct_calls(), 0);
    let refused = Setup::new(true).await;
    refused
        .message(|raw| raw["author"]["bot"] = true.into())
        .await;
    persist(&refused, &delete(&refused)).await;
    refused.sweep(refused.now()).await;
    assert!(plans(&refused).await.is_empty());
}

#[tokio::test]
async fn discord_bulk_delete_and_single_delete_share_target_identity() {
    let f = Setup::new(true).await;
    f.message(|_| {}).await;
    f.sweep(f.now()).await;
    let event = Dispatch {
        name: "MESSAGE_DELETE_BULK".into(),
        sequence: 5,
        data: json!({"ids":[f.fixture.message_id(),"700"],"channel_id":"99"}),
    };
    persist(&f, &event).await;
    persist(&f, &delete(&f)).await;
    f.sweep(f.now()).await;
    assert_eq!(plans(&f).await.len(), 2);
    let mut invalid = event;
    invalid.data["ids"] = json!([f.fixture.message_id(), f.fixture.message_id()]);
    persist(&f, &invalid).await;
    f.sweep(f.now()).await;
    assert_eq!(plans(&f).await.len(), 2);
}

#[tokio::test]
async fn discord_message_attachment_patch_retains_original_thread_and_exact_file_reference() {
    let mut f = Setup::new(true).await;
    let native = Arc::get_mut(&mut f.journal).unwrap();
    native.api.thread_metadata = true;
    f.fixture.memory.discord_channels.lock().unwrap().insert(
        "100".into(),
        json!({"id":"100","type":11,"guild_id":"88","parent_id":"99"}),
    );
    let id = f
        .message(|raw| {
            raw["channel_id"] = "100".into();
            raw["guild_id"] = "88".into();
        })
        .await;
    let (inbox, _) = f
        .fixture
        .store
        .load::<Inbox>(&id, true)
        .await
        .unwrap()
        .unwrap();
    let bytes = f
        .journal
        .verify_inbox(&inbox, &f.journal.authority, &f.fixture.message_id())
        .await
        .unwrap();
    let body: Body = serde_json::from_slice(&bytes).unwrap();
    let mut event = edit(&f);
    event.data["channel_id"] = "100".into();
    event.data["guild_id"] = "88".into();
    event.data.as_object_mut().unwrap().remove("content");
    event.data["attachments"] =
        json!([{"id":"800","filename":"note.txt","content_type":"text/plain","size":12}]);
    let snapshot = normalize_change(&body, "77", &event.name, &event.data, f.now())
        .unwrap()
        .unwrap();
    assert_eq!(
        snapshot.activity.conversation.thread_id.as_deref(),
        Some("100")
    );
    let ActivityKind::Event { ce, attachments } = snapshot.activity.kind else {
        panic!()
    };
    assert!(ce.data.as_ref().unwrap()["patch"].get("text").is_none());
    assert_eq!(attachments.len(), 1);
    assert_eq!(
        attachments[0].reference,
        format!("discord:{}:800", f.fixture.message_id())
    );
    f.sweep(f.now()).await;
    persist(&f, &event).await;
    assert_eq!(f.sweep(f.now()).await.pending, 0);
    let all = plans(&f).await;
    assert_eq!(all.len(), 2);
    assert!(all
        .iter()
        .all(|plan| plan.activity.conversation.thread_id.as_deref() == Some("100")));
}

#[tokio::test]
async fn discord_change_current_source_revocation_and_expiry_do_not_renew_work() {
    let f = Setup::new(true).await;
    f.message(|_| {}).await;
    f.sweep(f.now()).await;
    persist(&f, &edit(&f)).await;
    let mut revoked = f.channel.clone();
    revoked.secrets.discord_bot_token = Some(cmaeg_core::channel::Secret::new(
        "rotated-message-source-fixture",
    ));
    f.fixture.replace(&revoked);
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    assert_eq!(plans(&f).await.len(), 1);
    f.fixture.replace(&f.channel);
    assert_eq!(f.sweep(f.now() + 1800).await.pending, 0);
    assert_eq!(plans(&f).await.len(), 1);
}

#[tokio::test]
async fn discord_change_issuance_is_default_off_but_retained_recovery_survives_gate_off() {
    use super::{protocol::checkpoint, NativeDiscordJournal};
    let mut f = Setup::new(true).await;
    assert!(!f.journal.api.message_events);
    f.message(|_| {}).await;
    f.sweep(f.now()).await;
    f.journal.authorize_connection(None).await.unwrap();
    let ready = Dispatch {
        name: "READY".into(),
        sequence: 0,
        data: json!({"session_id":"change-gate-session","resume_gateway_url":"wss://gateway.discord.gg/","user":{"id":"77","bot":true}}),
    };
    let ready_checkpoint = checkpoint(None, &ready, "77").unwrap();
    f.journal.admit(&ready_checkpoint, &ready).await.unwrap();
    let mut event = edit(&f);
    let next = checkpoint(Some(&ready_checkpoint), &event, "77").unwrap();
    f.journal.admit(&next, &event).await.unwrap();
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    Arc::get_mut(&mut f.journal).unwrap().api.message_events = true;
    event.sequence += 1;
    let next = checkpoint(Some(&next), &event, "77").unwrap();
    let event_id = format!(
        "discord:77:{}:edit:{}",
        f.fixture.message_id(),
        i128::from(f.now()) * 1_000_000_000
    );
    let id = index_id(
        "discord-native-message-event-v1",
        &[&f.fixture.record, "77", &event_id],
    );
    f.fixture.fail(&id, false);
    assert!(f.journal.admit(&next, &event).await.is_err());
    assert_eq!(f.journal.checkpoint().await.unwrap().unwrap().sequence, 2);
    f.journal.admit(&next, &event).await.unwrap();
    assert_eq!(f.journal.checkpoint().await.unwrap().unwrap().sequence, 3);
    Arc::get_mut(&mut f.journal).unwrap().api.message_events = false;
    assert_eq!(f.sweep(f.now()).await.acknowledged, 2);
    assert_eq!(plans(&f).await.len(), 2);
}
