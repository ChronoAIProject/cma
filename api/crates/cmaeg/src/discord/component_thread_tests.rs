//! Thread control authority, current metadata and retained recovery over local HTTP.
use super::{
    components_fixture::{message, FixtureState},
    InboxLaunch, InboxReader,
};
use cmaeg_core::{activity::ActivityKind, seal::ReplyAddress};
use serde_json::{json, Value};

#[tokio::test]
async fn native_thread_components_preserve_exact_thread_and_single_consumer_through_replay() {
    for kind in [10, 11, 12] {
        for select in [false, true] {
            let f = FixtureState::named_thread(kind).await;
            let card = f.card(&message(select)).await;
            f.rest.memory.calls.lock().unwrap().clear();
            let mut raw = f.callback(
                card.markup[0]["components"][0]["custom_id"]
                    .as_str()
                    .unwrap(),
            );
            // A Gateway channel object is not the source of thread authority.
            raw.as_object_mut().unwrap().remove("channel");
            if select {
                raw["data"]["component_type"] = 3.into();
                raw["data"]["values"] = json!(["1"]);
            }
            let mut definition = f.setup.channel.to_blob_json();
            definition["routing"]["default_targets"] = json!(["default"]);
            definition["partition"]["key"] = "conversation".into();
            f.setup
                .fixture
                .replace(&serde_json::from_value(definition).unwrap());
            f.queue_thread_metadata();
            f.receive(raw.clone(), 2).await.unwrap();
            let current = f
                .setup
                .state
                .resolver
                .resolve_current(f.channel.id.as_str())
                .await
                .unwrap();
            let reader = InboxReader::new(f.setup.fixture.store.channels.clone(), f.keys());
            let InboxLaunch::Launch(first) = reader
                .load(&f.id(&raw), current.clone(), f.setup.now())
                .await
                .unwrap()
            else {
                panic!()
            };
            let activity = &first.request.activity;
            assert_eq!(activity.conversation.id, "99");
            assert_eq!(activity.conversation.thread_id.as_deref(), Some("99"));
            assert!(activity.thread.is_none());
            assert_eq!(first.request.conversation_id, f.original.conversation_id);
            assert_eq!(first.request.consumer, f.original.consumer);
            let routing = activity.routing.as_ref().unwrap();
            assert_eq!(routing.targets.len(), 1);
            assert_eq!(routing.targets[0].id, f.original.consumer.clone().unwrap());
            let ActivityKind::Interaction(result) = &activity.kind else {
                panic!()
            };
            assert_eq!(
                result.action_id,
                if select {
                    "private-write"
                } else {
                    "private-yes"
                }
            );
            let event = first
                .request
                .event_ref
                .open(&f.keys(), f.setup.now())
                .unwrap()
                .payload;
            assert_eq!(event.partition_key, f.original.partition_key);
            assert!(
                matches!(event.reply, ReplyAddress::Discord { channel_id, thread_id: Some(thread), message_id, .. } if channel_id == "99" && thread == "99" && message_id == "800")
            );
            assert!(
                !String::from_utf8_lossy(&first.source_bytes).contains("private-interaction-token")
            );
            {
                let calls = f.rest.memory.calls.lock().unwrap();
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].method, "GET");
                assert_eq!(calls[0].path, "/api/v10/channels/99");
                assert!(calls[0]
                    .authorization
                    .as_deref()
                    .unwrap()
                    .starts_with("Bot "));
                assert!(calls[1].path.ends_with("/callback"));
                assert!(calls[1].authorization.is_none());
                assert_eq!(
                    serde_json::from_slice::<Value>(&calls[1].body).unwrap(),
                    json!({"type":6})
                );
            }
            f.rest
                .respond(axum::http::StatusCode::SERVICE_UNAVAILABLE, Value::Null);
            f.receive(raw.clone(), 3).await.unwrap();
            let InboxLaunch::Launch(replay) = reader
                .load(&f.id(&raw), current, f.setup.now())
                .await
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(replay.source_bytes, first.source_bytes);
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn native_thread_callback_refuses_changed_thread_actor_message_and_metadata() {
    let cases = [
        ("/member/user/id", json!("43")),
        ("/channel_id", json!("100")),
        ("/message/id", json!("801")),
        ("/guild_id", json!("89")),
        ("/channel/type", json!(12)),
        ("/channel/parent_id", json!("101")),
        ("http:id", json!("100")),
        ("http:type", json!(0)),
        ("http:guild_id", json!("89")),
        ("http:parent_id", json!("101")),
        ("http:parent_id", Value::Null),
    ];
    for (field, value) in cases {
        let f = FixtureState::thread(11).await;
        let card = f.card(&message(false)).await;
        f.rest.memory.calls.lock().unwrap().clear();
        let mut raw = f.callback(
            card.markup[0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        );
        let mut metadata = f.thread_metadata();
        if let Some(field) = field.strip_prefix("http:") {
            metadata[field] = value;
        } else {
            *raw.pointer_mut(field).unwrap() = value;
        }
        f.rest.respond(axum::http::StatusCode::OK, metadata);
        f.receive(raw, 2).await.unwrap();
        assert!(
            f.store
                .load(&card.card.id)
                .await
                .unwrap()
                .0
                .consumed
                .is_none(),
            "{field}"
        );
        assert!(f
            .rest
            .memory
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call.method == "GET"));
        assert!(f.provider.memory.lock().unwrap().seen.is_empty());
    }
}

#[tokio::test]
async fn native_thread_consumed_callback_finishes_lost_inbox_after_deadline_without_new_read_or_ack(
) {
    let f = FixtureState::thread(11).await;
    let card = f.card(&message(false)).await;
    f.rest.memory.calls.lock().unwrap().clear();
    let raw = f.callback(
        card.markup[0]["components"][0]["custom_id"]
            .as_str()
            .unwrap(),
    );
    let id = f.id(&raw);
    f.setup.fixture.fail(&id, false);
    f.queue_thread_metadata();
    assert!(f.receive(raw.clone(), 2).await.is_err());
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_some());
    assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
    f.setup.fixture.advance(3000);
    f.rest
        .respond(axum::http::StatusCode::SERVICE_UNAVAILABLE, Value::Null);
    f.receive(raw, 2).await.unwrap();
    let InboxLaunch::Launch(restored) =
        InboxReader::new(f.setup.fixture.store.channels.clone(), f.keys())
            .load(&id, f.channel.clone(), f.setup.now())
            .await
            .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        restored.request.activity.conversation.thread_id.as_deref(),
        Some("99")
    );
    assert_eq!(restored.request.conversation_id, f.original.conversation_id);
    assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_thread_fresh_callback_requires_available_bounded_metadata_before_deadline() {
    use axum::http::StatusCode;
    for case in 0..5 {
        let f = FixtureState::thread(11).await;
        let card = f.card(&message(false)).await;
        f.rest.memory.calls.lock().unwrap().clear();
        let raw = f.callback(
            card.markup[0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        );
        match case {
            0 => f.rest.respond(StatusCode::FORBIDDEN, Value::Null),
            1 => f.rest.respond(StatusCode::SERVICE_UNAVAILABLE, Value::Null),
            2 => f
                .rest
                .respond(StatusCode::TOO_MANY_REQUESTS, json!({"retry_after":30})),
            3 => {
                let mut metadata = f.thread_metadata();
                metadata["unused"] = "x".repeat(65_537).into();
                f.rest.respond(StatusCode::OK, metadata);
            }
            _ => f.setup.fixture.advance(2500),
        }
        let result = f.receive(raw, 2).await;
        assert_eq!(result.is_err(), matches!(case, 1 | 2));
        assert!(f
            .store
            .load(&card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
        let calls = f.rest.memory.calls.lock().unwrap();
        assert_eq!(calls.len(), usize::from(case != 4));
        assert!(calls.iter().all(|call| call.method == "GET"));
        assert!(f.provider.memory.lock().unwrap().seen.is_empty());
    }
}

#[tokio::test]
async fn native_thread_controls_preserve_current_source_and_selected_consumer_revocation() {
    for source in [false, true] {
        let f = FixtureState::named_thread(11).await;
        let card = f.card(&message(false)).await;
        f.rest.memory.calls.lock().unwrap().clear();
        let mut definition = f.setup.channel.to_blob_json();
        if source {
            definition["secrets"]["discord_bot_token"] = "rotated-thread-fixture".into();
        } else {
            definition["routing"]["consumers"][0]["provider"]["binding_id"] = "bnd_replaced".into();
        }
        f.setup
            .fixture
            .replace(&serde_json::from_value(definition).unwrap());
        let raw = f.callback(
            card.markup[0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        );
        f.queue_thread_metadata();
        assert_eq!(f.receive(raw, 2).await.is_err(), source);
        assert!(f
            .store
            .load(&card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
        assert!(f
            .rest
            .memory
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call.method == "GET"));
        assert!(f.provider.memory.lock().unwrap().seen.is_empty());
    }
}
