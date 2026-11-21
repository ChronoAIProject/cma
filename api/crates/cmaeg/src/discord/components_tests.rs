use super::{
    components_fixture::{message, FixtureState},
    InboxLaunch, InboxReader,
};
use cmaeg_core::activity::ActivityKind;
use serde_json::{json, Value};

#[tokio::test]
async fn native_discord_consumed_click_recovers_failed_inbox_after_ack_deadline() {
    let f = FixtureState::new().await;
    let card = f.card(&message(false)).await;
    let raw = f.callback(
        card.markup[0]["components"][0]["custom_id"]
            .as_str()
            .unwrap(),
    );
    let id = f.id(&raw);
    f.setup.fixture.fail(&id, false);
    assert!(f.receive(raw.clone(), 2).await.is_err());
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_some());
    assert!(f.rest.memory.calls.lock().unwrap().is_empty());
    f.setup.fixture.advance(3000);
    f.receive(raw, 2).await.unwrap();
    let InboxLaunch::Launch(verified) = InboxReader::new(
        f.setup.fixture.store.channels.clone(),
        f.setup.fixture.store.keys.clone(),
    )
    .load(&id, f.channel.clone(), f.setup.now())
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(verified.request.conversation_id, f.original.conversation_id);
    assert!(f.rest.memory.calls.lock().unwrap().is_empty());
    assert_eq!(f.setup.sweep(f.setup.now()).await.pending, 0);
}

#[tokio::test]
async fn native_discord_named_card_keeps_original_consumer_conversation_after_routing_changes() {
    let f = FixtureState::named().await;
    f.settle_context_message().await;
    let card = f.card(&message(false)).await;
    let mut definition = f.setup.channel.to_blob_json();
    definition["routing"]["default_targets"] = json!(["default"]);
    definition["routing"]["consumers"][0]["name"] = "renamed".into();
    definition["partition"]["key"] = "fixed".into();
    f.setup
        .fixture
        .replace(&serde_json::from_value(definition).unwrap());
    let raw = f.callback(
        card.markup[0]["components"][0]["custom_id"]
            .as_str()
            .unwrap(),
    );
    f.receive(raw.clone(), 2).await.unwrap();
    let current = f
        .setup
        .state
        .resolver
        .resolve_current(f.channel.id.as_str())
        .await
        .unwrap();
    let InboxLaunch::Launch(verified) = InboxReader::new(
        f.setup.fixture.store.channels.clone(),
        f.setup.fixture.store.keys.clone(),
    )
    .load(&f.id(&raw), current, f.setup.now())
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(verified.request.consumer, f.original.consumer);
    assert_eq!(verified.request.conversation_id, f.original.conversation_id);
    assert_eq!(
        verified.request.activity.routing.as_ref().unwrap().targets[0].id,
        f.original.consumer.clone().unwrap()
    );
    assert_eq!(f.setup.sweep(f.setup.now()).await.pending, 0);
    let plan = f
        .setup
        .plans
        .replay_native_interaction(&verified.request, &verified.source_bytes)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(
        plan.targets[0].launch.consumer,
        f.original.consumer.clone().unwrap()
    );
    assert_eq!(
        plan.targets[0].launch.event.conversation_id,
        f.original.conversation_id
    );
}

#[tokio::test]
async fn native_discord_named_rebinding_and_source_rotation_revoke_card_authority() {
    for binding in [true, false] {
        let f = FixtureState::named().await;
        let card = f.card(&message(false)).await;
        let mut definition = f.setup.channel.to_blob_json();
        if binding {
            definition["routing"]["consumers"][0]["provider"]["binding_id"] = "bnd_replaced".into();
        } else {
            definition["secrets"]["discord_bot_token"] = "rotated-private-token".into();
        }
        f.setup
            .fixture
            .replace(&serde_json::from_value(definition).unwrap());
        let raw = f.callback(
            card.markup[0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        );
        let result = f.receive(raw, 2).await;
        if binding {
            result.unwrap();
        } else {
            assert!(result.is_err());
        }
        assert!(f
            .store
            .load(&card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
        assert!(f.rest.memory.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_discord_component_is_durable_before_type6_ack_and_replays_one_original_consumer() {
    let f = FixtureState::new().await;
    let card = f.card(&message(false)).await;
    let custom = card.markup[0]["components"][0]["custom_id"]
        .as_str()
        .unwrap();
    let raw = f.callback(custom);
    let records = f.setup.fixture.memory.clone();
    let expected = f.id(&raw);
    *f.rest.memory.on_call.lock().unwrap() = Some(std::sync::Arc::new(move |_| {
        assert!(records.rows.lock().unwrap().contains_key(&expected));
    }));
    f.receive(raw.clone(), 2).await.unwrap();
    let id = f.id(&raw);
    let reader = InboxReader::new(
        f.setup.fixture.store.channels.clone(),
        f.setup.fixture.store.keys.clone(),
    );
    let InboxLaunch::Launch(first) = reader
        .load(&id, f.channel.clone(), f.setup.now())
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(!String::from_utf8_lossy(&first.source_bytes).contains("private-interaction-token"));
    assert_eq!(first.request.conversation_id, f.original.conversation_id);
    assert_eq!(
        first
            .request
            .activity
            .routing
            .as_ref()
            .unwrap()
            .targets
            .len(),
        1
    );
    let ActivityKind::Interaction(interaction) = &first.request.activity.kind else {
        panic!()
    };
    assert_eq!(interaction.action_id, "private-yes");
    assert_eq!(interaction.values["selected"], json!(["private-yes"]));
    {
        let calls = f.rest.memory.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "POST");
        assert_eq!(
            calls[0].path,
            format!(
                "/api/v10/interactions/{}/private-interaction-token/callback",
                raw["id"].as_str().unwrap()
            )
        );
        assert!(calls[0].authorization.is_none());
        assert_eq!(
            serde_json::from_slice::<Value>(&calls[0].body).unwrap(),
            json!({"type":6})
        );
    }
    assert!(f.provider.memory.lock().unwrap().seen.is_empty());
    assert!(f.setup.sweep(f.setup.now()).await.acknowledged >= 2);
    f.receive(raw.clone(), 3).await.unwrap();
    assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
    assert_eq!(f.setup.direct_calls(), 0);
    let plan = f
        .setup
        .plans
        .replay_native_interaction(&first.request, &first.source_bytes)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(
        plan.targets[0].launch.event.conversation_id,
        f.original.conversation_id
    );
    assert!(f
        .setup
        .plans
        .replay(&first.request, &first.source_bytes)
        .await
        .is_err());
    assert!(f
        .setup
        .state
        .capture_routed(&first.request, b"unverified", true)
        .is_none());
}

#[tokio::test]
async fn native_discord_select_uses_only_exact_card_option_indices() {
    let f = FixtureState::new().await;
    let card = f.card(&message(true)).await;
    let custom = card.markup[0]["components"][0]["custom_id"]
        .as_str()
        .unwrap();
    for values in [
        json!(["private-write"]),
        json!(["01"]),
        json!(["2"]),
        json!(["0", "1"]),
    ] {
        let mut raw = f.callback(custom);
        raw["data"]["component_type"] = 3.into();
        raw["data"]["values"] = values;
        f.receive(raw, 2).await.unwrap();
        assert!(f
            .store
            .load(&card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
    }
    let mut raw = f.callback(custom);
    raw["data"]["component_type"] = 3.into();
    raw["data"]["values"] = json!(["1"]);
    f.receive(raw.clone(), 3).await.unwrap();
    let InboxLaunch::Launch(verified) = InboxReader::new(
        f.setup.fixture.store.channels.clone(),
        f.setup.fixture.store.keys.clone(),
    )
    .load(&f.id(&raw), f.channel.clone(), f.setup.now())
    .await
    .unwrap() else {
        panic!()
    };
    let ActivityKind::Interaction(value) = verified.request.activity.kind else {
        panic!()
    };
    assert_eq!(value.action_id, "private-write");
    assert_eq!(value.values["selected"], json!(["private-write"]));
}

#[tokio::test]
async fn native_discord_foreign_actor_bot_channel_message_revision_and_callback_kind_cannot_consume(
) {
    let f = FixtureState::new().await;
    let card = f.card(&message(false)).await;
    let custom = card.markup[0]["components"][0]["custom_id"]
        .as_str()
        .unwrap();
    for (pointer, value) in [
        ("/application_id", json!("78")),
        ("/user/id", json!("43")),
        ("/user/bot", json!(true)),
        ("/channel_id", json!("100")),
        ("/message/id", json!("801")),
        ("/message/author/id", json!("78")),
        ("/data/component_type", json!(5)),
        ("/type", json!(2)),
        ("/channel/type", json!(11)),
        ("/data/custom_id", json!(custom.replace(":1:", ":2:"))),
        ("/token", json!("bad/token")),
    ] {
        let mut raw = f.callback(custom);
        *raw.pointer_mut(pointer).unwrap() = value;
        f.receive(raw, 2).await.unwrap();
        assert!(
            f.store
                .load(&card.card.id)
                .await
                .unwrap()
                .0
                .consumed
                .is_none(),
            "{pointer}"
        );
    }
    assert!(f.rest.memory.calls.lock().unwrap().is_empty());
    assert!(f.provider.memory.lock().unwrap().seen.is_empty());
}

#[tokio::test]
async fn native_discord_lost_ack_is_not_retried_and_late_new_click_does_not_consume() {
    let f = FixtureState::new().await;
    let card = f.card(&message(false)).await;
    let custom = card.markup[0]["components"][0]["custom_id"]
        .as_str()
        .unwrap();
    let raw = f.callback(custom);
    f.rest
        .respond(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({}));
    f.receive(raw.clone(), 2).await.unwrap();
    f.receive(raw, 3).await.unwrap();
    assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
    let second = f.card(&message(false)).await;
    f.setup.fixture.advance(1);
    let raw = f.callback(
        second.markup[0]["components"][0]["custom_id"]
            .as_str()
            .unwrap(),
    );
    f.setup.fixture.advance(3000);
    f.receive(raw, 4).await.unwrap();
    assert!(f
        .store
        .load(&second.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
}
