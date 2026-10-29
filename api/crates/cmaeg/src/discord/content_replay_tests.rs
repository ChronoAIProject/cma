use super::{
    content_tests::{embed, poll, sticker},
    dispatch_fixture::Setup,
    inbox_scan::InboxStore,
    journal_fixture::{definition, Fixture},
    normalization_tests::{channel, raw},
    Dispatch, InboxLaunch, InboxReader,
};
use crate::connections::store::index_id;
use cmaeg_core::{activity::ActivityKind, channel::PartitionKey, seal::ReplyAddress};
use serde_json::{json, Value};
use std::sync::Arc;

#[tokio::test]
async fn new_snapshots_replay_with_writer_off_and_old_mixed_messages_never_change_digest() {
    for enabled in [false, true] {
        let mut f = Fixture::new().await;
        f.api.rich_content = enabled;
        let journal = f.acquire().await.unwrap();
        let mut data = raw();
        data["id"] = f.message_id().into();
        data["embeds"] = embed();
        data["sticker_items"] = sticker();
        data["poll"] = poll();
        let mut dispatch = Dispatch {
            name: "MESSAGE_CREATE".into(),
            sequence: 1,
            data,
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
        let now = (f.store.clock)() / 1000;
        let InboxLaunch::Launch(original) = reader.load(&id, channel(), now).await.unwrap() else {
            panic!()
        };
        let stored: Value = serde_json::from_slice(&original.source_bytes).unwrap();
        assert_eq!(stored.get("content_projection").is_some(), enabled);
        assert_eq!(stored["dispatch"], dispatch.data);
        let ActivityKind::Message { text, .. } = &original.request.activity.kind else {
            panic!()
        };
        assert_eq!(text.contains("\"kind\":\"poll\""), enabled);
        if !enabled {
            assert_eq!(text, "hello");
        }
        let opened = original
            .request
            .event_ref
            .open(&f.store.keys, now)
            .unwrap()
            .payload;
        assert!(opened.verifies_activity(&original.request.activity));
        journal.release().await.unwrap();
        drop(journal);
        f.api.rich_content = !enabled;
        let journal = f.acquire().await.unwrap();
        // Same native identity restores custody before today's parsing/gates.
        dispatch.sequence = 200;
        dispatch.data["embeds"] = json!([{}]);
        dispatch.data["poll"] = json!({"invalid":"new retry body"});
        dispatch.data["author"]["id"] = "43".into();
        dispatch.data["channel_id"] = "100".into();
        journal
            .persist_message(&journal.authority, &dispatch)
            .await
            .unwrap();
        let InboxLaunch::Launch(restored) = reader.load(&id, channel(), now).await.unwrap() else {
            panic!()
        };
        assert_eq!(original.source_bytes, restored.source_bytes);
        assert_eq!(original.request.activity, restored.request.activity);
        assert_eq!(
            original.request.conversation_id,
            restored.request.conversation_id
        );
        assert_eq!(
            original.request.idempotency_key,
            restored.request.idempotency_key
        );
        assert!(opened.verifies_activity(&restored.request.activity));
    }
}

#[tokio::test]
async fn standalone_rich_messages_keep_one_committed_routed_plan_after_lost_ack() {
    for (key, value) in [
        ("sticker_items", sticker()),
        ("embeds", embed()),
        ("poll", poll()),
    ] {
        let mut f = Setup::new(true).await;
        Arc::get_mut(&mut f.journal).unwrap().api.rich_content = true;
        let id = f
            .message(|raw| {
                raw["content"] = "".into();
                raw[key] = value;
            })
            .await;
        let store = InboxStore::new(
            f.fixture.store.channels.clone(),
            f.fixture.store.keys.clone(),
        );
        let page = store.page(None, f.now()).await.unwrap();
        let entry = page.entries.iter().find(|entry| entry.id == id).unwrap();
        f.fixture.fail(&entry.ack_id(), false);
        assert_eq!(f.sweep(f.now()).await.pending, 1);
        let before = f.plans.recover_all_page(None, 32).await.unwrap().plans;
        assert_eq!(before.len(), 1);
        Arc::get_mut(&mut f.journal).unwrap().api.rich_content = false;
        f.message(|raw| {
            raw["content"] = "conflicting retry".into();
        })
        .await;
        assert_eq!(f.sweep(f.now()).await.pending, 0);
        let after = f.plans.recover_all_page(None, 32).await.unwrap().plans;
        assert_eq!(before, after);
        assert_eq!(after[0].activity.actor.id, "42");
        assert_eq!(after[0].activity.conversation.id, "99");
        assert_eq!(f.direct_calls(), 0);
    }
}

#[tokio::test]
async fn rich_thread_messages_use_exact_retained_channel_evidence_and_fail_closed_without_it() {
    for verified in [false, true] {
        let mut f = Fixture::new().await;
        f.api.rich_content = true;
        f.api.thread_metadata = true;
        let mut definition = definition();
        definition.partition.key = PartitionKey::Thread;
        f.replace(&definition);
        if verified {
            f.memory.discord_channels.lock().unwrap().insert(
                "100".into(),
                json!({"id":"100","type":11,"guild_id":"88","parent_id":"99"}),
            );
        }
        let journal = f.acquire().await.unwrap();
        let mut data = raw();
        data["id"] = f.message_id().into();
        data["content"] = "".into();
        data["sticker_items"] = sticker();
        data["channel_id"] = "100".into();
        data["guild_id"] = "88".into();
        // This is a thread started from the message, never its current channel.
        data["thread"] = json!({"id":"101","type":11,"parent_id":"100","guild_id":"88"});
        let dispatch = Dispatch {
            name: "MESSAGE_CREATE".into(),
            sequence: 1,
            data,
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
        let now = (f.store.clock)() / 1000;
        let launch = reader.load(&id, channel(), now).await.unwrap();
        if !verified {
            assert!(matches!(
                launch,
                InboxLaunch::Refused(crate::ingress::admission::Refusal::ThreadEvidenceUnavailable)
            ));
            continue;
        }
        let InboxLaunch::Launch(launch) = launch else {
            panic!()
        };
        assert_eq!(launch.request.activity.conversation.id, "100");
        assert_eq!(
            launch.request.activity.conversation.thread_id.as_deref(),
            Some("100")
        );
        let reference = launch
            .request
            .event_ref
            .open(&f.store.keys, now)
            .unwrap()
            .payload;
        assert!(
            matches!(reference.reply, ReplyAddress::Discord { channel_id, thread_id: Some(thread_id), .. }
            if channel_id == "100" && thread_id == "100")
        );
        assert_eq!(*f.memory.channel_calls.lock().unwrap(), ["100"]);
    }
}

#[tokio::test]
async fn rich_metadata_updates_do_not_start_turns_and_edits_keep_original_consumers() {
    let mut f = Setup::new(true).await;
    Arc::get_mut(&mut f.journal).unwrap().api.rich_content = true;
    f.message(|raw| {
        raw["content"] = "".into();
        raw["poll"] = poll();
    })
    .await;
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    let original = f
        .plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .remove(0);
    for revision in [false, true] {
        let mut event = super::message_event_tests::edit(&f);
        event.data.as_object_mut().unwrap().remove("content");
        event.data["embeds"] = embed();
        event.data["poll"] = poll();
        if !revision {
            event
                .data
                .as_object_mut()
                .unwrap()
                .remove("edited_timestamp");
        }
        super::message_event_tests::persist(&f, &event).await;
    }
    f.sweep(f.now()).await;
    assert_eq!(
        f.plans.recover_all_page(None, 32).await.unwrap().plans,
        [original.clone()]
    );
    let edit = super::message_event_tests::edit(&f);
    super::message_event_tests::persist(&f, &edit).await;
    f.sweep(f.now()).await;
    let plans = f.plans.recover_all_page(None, 32).await.unwrap().plans;
    assert_eq!(plans.len(), 2);
    let change = plans
        .iter()
        .find(|plan| plan.identity != original.identity)
        .unwrap();
    assert_eq!(
        change.targets[0].launch.consumer,
        original.targets[0].launch.consumer
    );
    assert_eq!(
        change.targets[0].launch.event.conversation_id,
        original.targets[0].launch.event.conversation_id
    );
    assert!(
        matches!(&change.activity.kind, ActivityKind::Event { ce, .. }
        if ce.r#type == "cma.message.updated.v1" && ce.data.as_ref().unwrap()["patch"]["text"] == "edited instruction")
    );
    assert_eq!(f.direct_calls(), 0);
}

#[tokio::test]
async fn legacy_refused_rich_only_message_cannot_gain_admission_from_writer_activation() {
    let mut f = Setup::new(true).await;
    f.message(|raw| {
        raw["content"] = "".into();
        raw["sticker_items"] = sticker();
    })
    .await;
    // Activate before even the first dispatch: custody already pinned legacy semantics.
    Arc::get_mut(&mut f.journal).unwrap().api.rich_content = true;
    f.message(|raw| {
        raw["content"] = "".into();
        raw["sticker_items"] = sticker();
    })
    .await;
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    assert!(f
        .plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .is_empty());
    assert_eq!(f.direct_calls(), 0);
}
