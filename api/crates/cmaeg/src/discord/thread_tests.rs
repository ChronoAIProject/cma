use super::{
    dispatch_fixture::Setup,
    inbox_scan::InboxStore,
    journal_fixture::{definition, Fixture},
    normalization_tests::{channel, raw},
    Dispatch, GatewayError, InboxLaunch, InboxReader, VerifiedInboxLaunch,
};
use crate::{connections::store::index_id, ingress::admission::Refusal};
use cmaeg_core::{
    channel::{AdmissionPolicy, PartitionKey, Source},
    seal::ReplyAddress,
};
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc};

fn metadata(f: &Fixture, id: &str, value: Value) {
    f.memory
        .discord_channels
        .lock()
        .unwrap()
        .insert(id.into(), value);
}
fn thread(id: &str, kind: u8) -> Value {
    json!({"id":id,"type":kind,"parent_id":"99","guild_id":"88"})
}
async fn fixture() -> Fixture {
    let mut f = Fixture::new().await;
    f.api = f.api.clone().with_thread_metadata(true);
    let mut original = definition();
    original.partition.key = PartitionKey::Thread;
    f.replace(&original);
    f
}
fn dispatch(f: &Fixture, id: &str) -> Dispatch {
    let mut data = raw();
    data["id"] = f.message_id().into();
    data["channel_id"] = id.into();
    data["guild_id"] = "88".into();
    Dispatch {
        name: "MESSAGE_CREATE".into(),
        sequence: 1,
        data,
    }
}
fn inbox_id(f: &Fixture, event: &Dispatch) -> String {
    index_id(
        "discord-native-message-v1",
        &[&f.record, "77", event.message_id().unwrap()],
    )
}
async fn load(f: &Fixture, event: &Dispatch) -> InboxLaunch {
    InboxReader::new(f.store.channels.clone(), f.store.keys.clone())
        .load(&inbox_id(f, event), channel(), (f.store.clock)() / 1000)
        .await
        .unwrap()
}
fn launched(launch: InboxLaunch) -> Box<VerifiedInboxLaunch> {
    let InboxLaunch::Launch(launch) = launch else {
        panic!("expected verified native launch")
    };
    launch
}

#[tokio::test]
async fn parent_message_started_thread_is_not_current_thread_and_all_thread_kinds_are_exact() {
    let f = fixture().await;
    let journal = f.acquire().await.unwrap();
    metadata(
        &f,
        "99",
        json!({"id":"99","type":0,"guild_id":"88","parent_id":"90"}),
    );
    let mut parent = dispatch(&f, "99");
    parent.data["thread"] = thread("100", 11);
    journal
        .persist_message(&journal.authority, &parent)
        .await
        .unwrap();
    let parent = launched(load(&f, &parent).await);
    assert_eq!(parent.request.activity.conversation.id, "99");
    assert_eq!(parent.request.activity.conversation.thread_id, None);
    let mut conversations = std::collections::BTreeSet::new();
    conversations.insert(parent.request.conversation_id.to_string());
    for (id, kind) in [("100", 10), ("101", 11), ("102", 12)] {
        f.advance(1_000);
        metadata(&f, id, thread(id, kind));
        let event = dispatch(&f, id);
        journal
            .persist_message(&journal.authority, &event)
            .await
            .unwrap();
        let launch = launched(load(&f, &event).await);
        let activity = &launch.request.activity;
        assert_eq!(activity.conversation.id, id);
        assert_eq!(activity.conversation.thread_id.as_deref(), Some(id));
        assert!(
            activity.thread.is_none(),
            "Lark thread facts must not be fabricated"
        );
        assert!(conversations.insert(launch.request.conversation_id.to_string()));
        let reference = launch
            .request
            .event_ref
            .open(&f.store.keys, (f.store.clock)() / 1000)
            .unwrap()
            .payload;
        assert!(
            matches!(reference.reply, ReplyAddress::Discord { channel_id, thread_id: Some(thread_id), .. }
            if channel_id == id && thread_id == id)
        );
        assert!(
            !String::from_utf8_lossy(&launch.source_bytes).contains(super::journal_fixture::TOKEN)
        );
    }
    assert_eq!(
        &*f.memory.channel_calls.lock().unwrap(),
        &["99", "100", "101", "102"]
    );
}

#[tokio::test]
async fn metadata_failure_or_mismatch_never_silently_changes_thread_partition() {
    for response in [
        None,
        Some(thread("999", 11)),
        Some(json!({"id":"100","type":11,"guild_id":"89","parent_id":"99"})),
        Some(json!({"id":"100","type":11,"guild_id":"88"})),
        Some(json!({"id":"100","type":11,"guild_id":"88","parent_id":"100"})),
        Some(json!({"id":"100","type":11,"guild_id":"88","parent_id":"099"})),
        Some(json!({"id":"100","type":99,"guild_id":"88","parent_id":"99"})),
    ] {
        let f = fixture().await;
        let journal = f.acquire().await.unwrap();
        if let Some(response) = response {
            metadata(&f, "100", response);
        }
        let mut event = dispatch(&f, "100");
        event.data["thread"] = thread("100", 11); // Never accepted as alternate evidence.
        journal
            .persist_message(&journal.authority, &event)
            .await
            .unwrap();
        assert!(matches!(
            load(&f, &event).await,
            InboxLaunch::Refused(Refusal::ThreadEvidenceUnavailable)
        ));
        metadata(&f, "100", thread("100", 11));
        journal
            .persist_message(&journal.authority, &event)
            .await
            .unwrap();
        assert!(matches!(
            load(&f, &event).await,
            InboxLaunch::Refused(Refusal::ThreadEvidenceUnavailable)
        ));
        assert_eq!(
            f.memory.channel_calls.lock().unwrap().len(),
            1,
            "replay must use original proof outcome"
        );
    }
}

#[tokio::test]
async fn authenticated_group_metadata_cannot_reuse_private_message_admission_facts() {
    let f = fixture().await;
    let journal = f.acquire().await.unwrap();
    metadata(&f, "100", json!({"id":"100","type":3}));
    let mut event = dispatch(&f, "100");
    event.data.as_object_mut().unwrap().remove("guild_id");
    journal
        .persist_message(&journal.authority, &event)
        .await
        .unwrap();
    assert!(matches!(
        load(&f, &event).await,
        InboxLaunch::Refused(Refusal::ThreadEvidenceUnavailable)
    ));
    f.advance(1_000);
    event.data["id"] = f.message_id().into();
    event.data["channel_type"] = 3.into();
    journal
        .persist_message(&journal.authority, &event)
        .await
        .unwrap();
    assert_eq!(
        launched(load(&f, &event).await)
            .request
            .activity
            .conversation
            .kind,
        cmaeg_core::activity::ConversationKind::Group
    );
}

#[tokio::test]
async fn metadata_rate_limit_is_bounded_without_retry_or_partition_fallback() {
    let f = fixture().await;
    let journal = f.acquire().await.unwrap();
    metadata(&f, "100", thread("100", 11));
    *f.memory.channel_status.lock().unwrap() = axum::http::StatusCode::TOO_MANY_REQUESTS;
    let first = dispatch(&f, "100");
    journal
        .persist_message(&journal.authority, &first)
        .await
        .unwrap();
    f.advance(1_000);
    let second = dispatch(&f, "100");
    journal
        .persist_message(&journal.authority, &second)
        .await
        .unwrap();
    assert_eq!(f.memory.channel_calls.lock().unwrap().len(), 1);
    for event in [first, second] {
        assert!(matches!(
            load(&f, &event).await,
            InboxLaunch::Refused(Refusal::ThreadEvidenceUnavailable)
        ));
    }
}

#[tokio::test]
async fn unauthorized_unavailable_and_oversized_metadata_are_explicit_thread_refusals() {
    for (status, oversized) in [
        (axum::http::StatusCode::FORBIDDEN, false),
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, false),
        (axum::http::StatusCode::OK, true),
    ] {
        let f = fixture().await;
        let journal = f.acquire().await.unwrap();
        let mut body = thread("100", 11);
        if oversized {
            body["unrelated"] = "x".repeat(65_536).into();
        }
        metadata(&f, "100", body);
        *f.memory.channel_status.lock().unwrap() = status;
        let event = dispatch(&f, "100");
        journal
            .persist_message(&journal.authority, &event)
            .await
            .unwrap();
        assert!(matches!(
            load(&f, &event).await,
            InboxLaunch::Refused(Refusal::ThreadEvidenceUnavailable)
        ));
        assert_eq!(f.memory.channel_calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn retained_thread_proof_survives_gate_and_partition_changes_but_source_revocation_refuses() {
    let f = fixture().await;
    let mut journal = f.acquire().await.unwrap();
    metadata(&f, "100", thread("100", 11));
    f.memory.channel_elapsed_ms.store(2_000, Ordering::SeqCst);
    let received = (f.store.clock)() / 1000;
    let event = dispatch(&f, "100");
    journal
        .persist_message(&journal.authority, &event)
        .await
        .unwrap();
    let first = launched(load(&f, &event).await);
    assert_eq!(first.received_at, received);
    assert_eq!(
        first
            .request
            .event_ref
            .open(&f.store.keys, received + 2)
            .unwrap()
            .payload
            .expires_at,
        received + 1800
    );
    assert_eq!((f.store.clock)() / 1000, received + 2);
    let mut changed = definition();
    changed.partition.key = PartitionKey::Conversation;
    f.replace(&changed);
    Arc::get_mut(&mut journal).unwrap().api.thread_metadata = false;
    metadata(&f, "100", json!({"id":"100","type":0,"guild_id":"88"}));
    journal
        .persist_message(&journal.authority, &event)
        .await
        .unwrap();
    let replay = launched(load(&f, &event).await);
    assert_eq!(first.source_bytes, replay.source_bytes);
    assert_eq!(
        first.request.conversation_id,
        replay.request.conversation_id
    );
    assert_eq!(first.received_at, replay.received_at);
    assert_eq!(f.memory.channel_calls.lock().unwrap().len(), 1);
    let Source::Discord { admission, .. } = &mut changed.sources[0] else {
        panic!()
    };
    *admission = serde_json::from_value::<AdmissionPolicy>(json!({
        "type":"scoped", "senders":{"type":"open"}, "chats":{"type":"open"}, "groups":"mention"
    }))
    .unwrap();
    f.replace(&changed);
    let mut current = channel();
    Arc::get_mut(&mut current).unwrap().definition = changed;
    assert!(matches!(
        InboxReader::new(f.store.channels.clone(), f.store.keys.clone())
            .load(&inbox_id(&f, &event), current, (f.store.clock)() / 1000)
            .await,
        Err(GatewayError::Authority)
    ));
}

#[tokio::test]
async fn verified_thread_receipt_precedes_capacity_and_retry_keeps_one_plan_and_activity() {
    let mut f = Setup::new(true).await;
    f.channel.partition.key = PartitionKey::Thread;
    f.fixture.replace(&f.channel);
    let journal = Arc::get_mut(&mut f.journal).unwrap();
    journal.api.thread_metadata = true;
    journal.authority.public_definition["partition"]["key"] = "thread".into();
    metadata(&f.fixture, "100", thread("100", 11));
    f.fixture
        .memory
        .channel_elapsed_ms
        .store(2_000, Ordering::SeqCst);
    let activity = f.observe();
    let mut changes = activity.subscribe();
    let capacity = f
        .state
        .inflight
        .clone()
        .acquire_many_owned(f.state.inflight.available_permits() as u32)
        .await
        .unwrap();
    let received = f.now();
    let id = f
        .message(|value| {
            value["channel_id"] = "100".into();
            value["guild_id"] = "88".into();
        })
        .await;
    assert_eq!(changes.try_recv().unwrap(), f.fixture.record);
    let before = activity
        .latest(&f.channel.owner.subject, &f.fixture.record)
        .await
        .unwrap();
    assert_eq!(before.items.len(), 1);
    let time = time::OffsetDateTime::from_unix_timestamp(received)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    assert_eq!(before.items[0].received_at, time);
    assert!(before.items[0].consumers.is_empty());
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    assert!(f
        .plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .is_empty());
    drop(capacity);
    let store = InboxStore::new(
        f.fixture.store.channels.clone(),
        f.fixture.store.keys.clone(),
    );
    let page = store.page(None, f.now()).await.unwrap();
    f.fixture.fail(&page.entries[0].ack_id(), false);
    assert_eq!(f.sweep(f.now()).await.pending, 1);
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    assert_eq!(f.sweep(f.now()).await.acknowledged, 1);
    let plans = f.plans.recover_all_page(None, 32).await.unwrap().plans;
    assert_eq!(plans.len(), 1);
    assert_eq!(f.direct_calls(), 0);
    let after = activity
        .latest(&f.channel.owner.subject, &f.fixture.record)
        .await
        .unwrap();
    assert_eq!(after.items.len(), 1);
    assert_eq!(after.items[0].received_at, before.items[0].received_at);
    assert!(f
        .fixture
        .store
        .load::<Value>(&id, true)
        .await
        .unwrap()
        .is_some());
    assert_eq!(f.fixture.memory.channel_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn disabled_gate_and_refused_senders_make_no_channel_request_and_ordinary_delivery_survives()
{
    let f = Fixture::new().await;
    let mut journal = f.acquire().await.unwrap();
    let event = dispatch(&f, "99");
    journal
        .persist_message(&journal.authority, &event)
        .await
        .unwrap();
    let ordinary = launched(load(&f, &event).await);
    assert!(serde_json::from_slice::<Value>(&ordinary.source_bytes)
        .unwrap()
        .get("native_channel")
        .is_none());
    Arc::get_mut(&mut journal).unwrap().api.thread_metadata = true;
    f.advance(1_000);
    let mut bot = dispatch(&f, "99");
    bot.data["author"]["bot"] = true.into();
    journal
        .persist_message(&journal.authority, &bot)
        .await
        .unwrap();
    assert!(matches!(
        load(&f, &bot).await,
        InboxLaunch::Refused(Refusal::BotSender)
    ));
    assert!(f.memory.channel_calls.lock().unwrap().is_empty());
    f.advance(1_000);
    let event = dispatch(&f, "99");
    journal
        .persist_message(&journal.authority, &event)
        .await
        .unwrap();
    assert!(
        matches!(load(&f, &event).await, InboxLaunch::Launch(_)),
        "ordinary non-thread partition retains its behavior on unavailable metadata"
    );
}

#[tokio::test]
async fn scoped_discord_thread_admission_uses_the_current_exact_conversation() {
    let f = fixture().await;
    let mut saved = definition();
    saved.partition.key = PartitionKey::Thread;
    let Source::Discord { admission, .. } = &mut saved.sources[0] else {
        panic!("expected native Discord source");
    };
    *admission = serde_json::from_value(json!({
        "type":"scoped","senders":{"type":"allowlist","ids":["42"]},
        "chats":{"type":"allowlist","ids":["100"]},"groups":"all"
    }))
    .unwrap();
    f.replace(&saved);
    let journal = f.acquire().await.unwrap();
    let reader = InboxReader::new(f.store.channels.clone(), f.store.keys.clone());
    let mut current = channel();
    Arc::get_mut(&mut current).unwrap().definition = saved.clone();
    metadata(&f, "100", thread("100", 11));
    metadata(&f, "101", thread("101", 11));

    for id in ["99", "100", "101"] {
        f.advance(1_000);
        let event = dispatch(&f, id);
        journal
            .persist_message(&journal.authority, &event)
            .await
            .unwrap();
        let result = reader
            .load(
                &inbox_id(&f, &event),
                current.clone(),
                (f.store.clock)() / 1000,
            )
            .await
            .unwrap();
        if id == "100" {
            let activity = launched(result).request.activity;
            assert_eq!(activity.conversation.id, "100");
            assert_eq!(activity.conversation.thread_id.as_deref(), Some("100"));
        } else {
            assert!(matches!(result, InboxLaunch::Refused(Refusal::ChatDenied)));
        }
    }

    journal.release().await.unwrap();
    let Source::Discord { admission, .. } = &mut saved.sources[0] else {
        unreachable!()
    };
    *admission = serde_json::from_value(json!({
        "type":"scoped","senders":{"type":"allowlist","ids":["42"]},
        "chats":{"type":"allowlist","ids":["99"]},"groups":"all"
    }))
    .unwrap();
    f.replace(&saved);
    let journal = f.acquire().await.unwrap();
    Arc::get_mut(&mut current).unwrap().definition = saved;
    metadata(
        &f,
        "99",
        json!({"id":"99","type":0,"guild_id":"88","parent_id":"90"}),
    );

    for id in ["99", "100"] {
        f.advance(1_000);
        let event = dispatch(&f, id);
        journal
            .persist_message(&journal.authority, &event)
            .await
            .unwrap();
        let result = reader
            .load(
                &inbox_id(&f, &event),
                current.clone(),
                (f.store.clock)() / 1000,
            )
            .await
            .unwrap();
        if id == "99" {
            let activity = launched(result).request.activity;
            assert_eq!(activity.conversation.id, "99");
            assert!(activity.conversation.thread_id.is_none());
        } else {
            assert!(matches!(result, InboxLaunch::Refused(Refusal::ChatDenied)));
        }
    }
}
