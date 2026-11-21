use super::{
    dispatch_fixture::Setup,
    forward_tests::forwarded,
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
async fn inbox_pins_forward_issuance_across_restart_and_preserves_legacy_rich_v1_bytes() {
    for enabled in [false, true] {
        for rich in [false, true] {
            let mut f = Fixture::new().await;
            f.api.forwards = enabled;
            f.api.rich_content = rich;
            let journal = f.acquire().await.unwrap();
            let mut data = raw();
            forwarded(&mut data);
            data["id"] = f.message_id().into();
            if rich {
                data["embeds"] = super::content_tests::embed();
            }
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
            let InboxLaunch::Launch(original) = reader.load(&id, channel(), now).await.unwrap()
            else {
                panic!()
            };
            let stored: Value = serde_json::from_slice(&original.source_bytes).unwrap();
            assert_eq!(stored.get("forward_projection").is_some(), enabled);
            assert_eq!(stored.get("content_projection").is_some(), rich);
            assert_eq!(stored["dispatch"], dispatch.data);
            if rich {
                assert_eq!(stored["content_projection"]["version"], 1);
            }
            assert!(
                matches!(&original.request.activity.kind, ActivityKind::Message { text, .. } if text.contains("Original words") == enabled)
            );
            let sealed = original
                .request
                .event_ref
                .open(&f.store.keys, now)
                .unwrap()
                .payload;
            journal.release().await.unwrap();
            drop(journal);
            f.api.forwards = !enabled;
            f.api.rich_content = !rich;
            let restarted = f.acquire().await.unwrap();
            restarted
                .persist_message(&restarted.authority, &dispatch)
                .await
                .unwrap();
            let InboxLaunch::Launch(restored) = reader.load(&id, channel(), now).await.unwrap()
            else {
                panic!()
            };
            assert_eq!(original.source_bytes, restored.source_bytes);
            assert_eq!(original.request.activity, restored.request.activity);
            assert_eq!(
                original.request.idempotency_key,
                restored.request.idempotency_key
            );
            assert_eq!(
                original.request.conversation_id,
                restored.request.conversation_id
            );
            assert!(sealed.verifies_activity(&restored.request.activity));
            assert!(f.memory.channel_calls.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn old_forward_only_refusal_cannot_gain_work_when_writer_activates() {
    let mut f = Setup::new(true).await;
    f.message(|raw| {
        forwarded(raw);
        raw["content"] = "".into();
    })
    .await;
    Arc::get_mut(&mut f.journal).unwrap().api.forwards = true;
    f.message(|raw| {
        forwarded(raw);
        raw["content"] = "".into();
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

#[tokio::test]
async fn projected_forward_retries_reject_snapshot_or_identity_mutations_without_new_custody() {
    let mut f = Fixture::new().await;
    f.api.forwards = true;
    let journal = f.acquire().await.unwrap();
    let mut data = raw();
    forwarded(&mut data);
    data["id"] = f.message_id().into();
    data["content"] = "".into();
    let original = Dispatch {
        name: "MESSAGE_CREATE".into(),
        sequence: 1,
        data,
    };
    journal
        .persist_message(&journal.authority, &original)
        .await
        .unwrap();
    let id = index_id(
        "discord-native-message-v1",
        &[&f.record, "77", original.message_id().unwrap()],
    );
    let reader = InboxReader::new(f.store.channels.clone(), f.store.keys.clone());
    let now = (f.store.clock)() / 1000;
    let InboxLaunch::Launch(before) = reader.load(&id, channel(), now).await.unwrap() else {
        panic!()
    };
    for index in 0..7 {
        let mut data = original.data.clone();
        match index {
            0 => data["message_snapshots"][0]["message"]["content"] = "replacement".into(),
            1 => data["message_reference"]["channel_id"] = "999".into(),
            2 => data["message_reference"]["guild_id"] = "999".into(),
            3 => data["message_reference"]["type"] = 0.into(),
            4 => data["author"]["id"] = "43".into(),
            5 => data["channel_id"] = "100".into(),
            _ => data["guild_id"] = "88".into(),
        }
        let retry = Dispatch {
            name: "MESSAGE_CREATE".into(),
            sequence: 2,
            data,
        };
        assert!(journal
            .persist_message(&journal.authority, &retry)
            .await
            .is_err());
    }
    let InboxLaunch::Launch(after) = reader.load(&id, channel(), now).await.unwrap() else {
        panic!()
    };
    assert_eq!(before.source_bytes, after.source_bytes);
    assert_eq!(before.request.activity, after.request.activity);
    assert!(f.memory.channel_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn forward_lost_ack_and_gate_off_retry_keep_exact_default_and_named_consumers() {
    let mut f = Setup::new(true).await;
    let mut value = f.channel.to_blob_json();
    let routed = crate::channels::routing_fixture::routed();
    value["routing"]["consumers"] = json!([routed["routing"]["consumers"][0]]);
    value["secrets"]["consumer_keys"] = json!([routed["secrets"]["consumer_keys"][0]]);
    value["routing"]["default_targets"] =
        json!(["default", crate::channels::routing_fixture::REVIEWER]);
    f.channel = serde_json::from_value(value).unwrap();
    f.fixture.replace(&f.channel);
    let journal = Arc::get_mut(&mut f.journal).unwrap();
    journal.api.forwards = true;
    journal.authority.public_definition = f.channel.to_public_json();
    let id = f
        .message(|raw| {
            forwarded(raw);
            raw["content"] = "".into();
            raw["message_snapshots"][0]["message"]["content"] =
                "@reviewer untrusted nested routing request".into();
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
    assert_eq!(before[0].targets.len(), 2);
    assert_ne!(
        before[0].targets[0].launch.consumer,
        before[0].targets[1].launch.consumer
    );
    assert_ne!(
        before[0].targets[0].launch.event.conversation_id,
        before[0].targets[1].launch.event.conversation_id
    );
    for target in &before[0].targets {
        assert!(target.launch.event.verifies_activity(&before[0].activity));
        assert!(
            matches!(&target.launch.event.reply, ReplyAddress::Discord { channel_id, .. } if channel_id == "99")
        );
    }
    Arc::get_mut(&mut f.journal).unwrap().api.forwards = false;
    f.message(|raw| {
        forwarded(raw);
        raw["content"] = "".into();
        raw["message_snapshots"][0]["message"]["content"] =
            "@reviewer untrusted nested routing request".into();
    })
    .await;
    assert_eq!(f.sweep(f.now()).await.pending, 0);
    assert_eq!(
        before,
        f.plans.recover_all_page(None, 32).await.unwrap().plans
    );
    assert_eq!(f.direct_calls(), 0);
    assert!(f.fixture.memory.channel_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn forwarded_origin_and_nested_metadata_never_supply_current_thread_evidence() {
    for verified in [false, true] {
        let mut f = Fixture::new().await;
        f.api.forwards = true;
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
        forwarded(&mut data);
        data["id"] = f.message_id().into();
        data["content"] = "".into();
        data["channel_id"] = "100".into();
        data["guild_id"] = "88".into();
        data["thread"] = json!({"id":"456","type":11,"guild_id":"789","parent_id":"123"});
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
        let launch = reader
            .load(&id, channel(), (f.store.clock)() / 1000)
            .await
            .unwrap();
        if !verified {
            assert!(matches!(
                launch,
                InboxLaunch::Refused(crate::ingress::admission::Refusal::ThreadEvidenceUnavailable)
            ));
        } else {
            let InboxLaunch::Launch(launch) = launch else {
                panic!()
            };
            assert_eq!(
                launch.request.activity.conversation.thread_id.as_deref(),
                Some("100")
            );
        }
        assert_eq!(*f.memory.channel_calls.lock().unwrap(), ["100"]);
    }
}

#[tokio::test]
async fn snapshot_metadata_updates_do_not_create_turns_or_replace_original_contents() {
    let mut f = Setup::new(true).await;
    Arc::get_mut(&mut f.journal).unwrap().api.forwards = true;
    f.message(|raw| {
        forwarded(raw);
        raw["content"] = "".into();
    })
    .await;
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    let before = f.plans.recover_all_page(None, 32).await.unwrap().plans;
    for with_revision in [false, true] {
        let mut event = super::message_event_tests::edit(&f);
        event.data.as_object_mut().unwrap().remove("content");
        event.data["message_snapshots"] = json!([{"message":{"content":"mutated original"}}]);
        if !with_revision {
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
        before,
        f.plans.recover_all_page(None, 32).await.unwrap().plans
    );
    assert_eq!(f.direct_calls(), 0);
}
