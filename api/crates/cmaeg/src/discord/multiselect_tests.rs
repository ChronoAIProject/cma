//! Complete selection sets retain the same durable native-card authority.
use super::{
    components_fixture::{message, multiselect_message, FixtureState},
    InboxLaunch, InboxReader,
};
use cmaeg_core::{activity::ActivityKind, message::Block};
use serde_json::{json, Value};

async fn selected(f: &FixtureState, raw: &Value) -> Value {
    let InboxLaunch::Launch(verified) =
        InboxReader::new(f.setup.fixture.store.channels.clone(), f.keys())
            .load(&f.id(raw), f.channel.clone(), f.setup.now())
            .await
            .unwrap()
    else {
        panic!()
    };
    assert_eq!(verified.request.conversation_id, f.original.conversation_id);
    assert_eq!(verified.request.consumer, f.original.consumer);
    assert_eq!(
        verified.request.activity.conversation,
        f.context.activity.conversation
    );
    assert!(!String::from_utf8(verified.source_bytes)
        .unwrap()
        .contains("private-interaction-token"));
    let ActivityKind::Interaction(result) = verified.request.activity.kind else {
        panic!()
    };
    assert_eq!(result.block_id, "scope");
    assert_eq!(result.action_id, "scope");
    result.values["selected"].clone()
}

#[tokio::test]
async fn native_discord_multiselect_submits_canonical_whole_sets_including_clear() {
    for (values, expected, indices) in [
        (
            json!(["1", "0"]),
            json!(["private-read", "private-write"]),
            json!([0, 1]),
        ),
        (json!(["1"]), json!(["private-write"]), json!([1])),
        (json!([]), json!([]), json!([])),
    ] {
        let f = FixtureState::named().await;
        let card = f.card(&multiselect_message()).await;
        let mut raw = f.callback(
            card.markup[0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        );
        raw["data"]["component_type"] = 3.into();
        raw["data"]["values"] = values;
        f.receive(raw.clone(), 2).await.unwrap();
        assert_eq!(selected(&f, &raw).await, expected);
        let saved = f.store.load(&card.card.id).await.unwrap().0;
        assert_eq!(
            serde_json::to_value(saved).unwrap()["consumed"]["selection"],
            indices
        );
        f.receive(raw.clone(), 3).await.unwrap();
        assert_eq!(selected(&f, &raw).await, expected);
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn native_discord_multiselect_refuses_malformed_duplicate_and_cross_block_values() {
    let f = FixtureState::new().await;
    let mut outbound = multiselect_message();
    outbound
        .blocks
        .insert(0, message(false).blocks.into_iter().next().unwrap());
    let card = f.card(&outbound).await;
    let custom = card.markup[1]["components"][0]["custom_id"]
        .as_str()
        .unwrap();
    for values in [
        json!(["private-read"]),
        json!(["02"]),
        json!(["-1"]),
        json!(["2", "2"]),
        json!(["0"]),
        json!(["4"]),
        json!([2]),
        json!(vec!["2"; 21]),
        Value::Null,
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
    f.receive(raw.clone(), 2).await.unwrap(); // Missing is not an explicit empty set.
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    raw["data"]["values"] = json!(["3", "2"]);
    f.receive(raw.clone(), 3).await.unwrap();
    assert_eq!(
        selected(&f, &raw).await,
        json!(["private-read", "private-write"])
    );
    assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_discord_multiselect_lost_inbox_requires_original_set_after_deadline() {
    let f = FixtureState::named_thread(11).await;
    let card = f.card(&multiselect_message()).await;
    let mut raw = f.callback(
        card.markup[0]["components"][0]["custom_id"]
            .as_str()
            .unwrap(),
    );
    raw["data"]["component_type"] = 3.into();
    raw["data"]["values"] = json!(["1", "0"]);
    f.queue_thread_metadata();
    f.setup.fixture.fail(&f.id(&raw), false);
    assert!(f.receive(raw.clone(), 2).await.is_err());
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_some());
    let reads = f.rest.memory.calls.lock().unwrap().len();
    f.setup.fixture.advance(3000);
    let mut changed = raw.clone();
    changed["data"]["values"] = json!(["1"]);
    f.receive(changed, 2).await.unwrap();
    assert!(
        InboxReader::new(f.setup.fixture.store.channels.clone(), f.keys())
            .load(&f.id(&raw), f.channel.clone(), f.setup.now())
            .await
            .is_err()
    );
    raw["data"]["values"] = json!(["0", "1"]); // Same set, canonical card order.
    f.receive(raw.clone(), 2).await.unwrap();
    assert_eq!(
        selected(&f, &raw).await,
        json!(["private-read", "private-write"])
    );
    assert_eq!(f.rest.memory.calls.lock().unwrap().len(), reads);
}

#[tokio::test]
async fn native_discord_multiselect_retains_actor_message_thread_and_revision_fences() {
    let f = FixtureState::named_thread(11).await;
    let card = f.card(&multiselect_message()).await;
    let mut original = f.callback(
        card.markup[0]["components"][0]["custom_id"]
            .as_str()
            .unwrap(),
    );
    original["data"]["component_type"] = 3.into();
    original["data"]["values"] = json!(["0", "1"]);
    for pointer in [
        "/member/user/id",
        "/message/id",
        "/channel_id",
        "/channel/parent_id",
        "/guild_id",
        "/application_id",
    ] {
        let mut raw = original.clone();
        *raw.pointer_mut(pointer).unwrap() = "555".into();
        f.queue_thread_metadata();
        f.receive(raw, 2).await.unwrap();
        assert!(f
            .store
            .load(&card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
        f.rest.memory.responses.lock().unwrap().clear();
    }
    let mut wrong_revision = original.clone();
    wrong_revision["data"]["custom_id"] = wrong_revision["data"]["custom_id"]
        .as_str()
        .unwrap()
        .replace(":1:", ":2:")
        .into();
    f.queue_thread_metadata();
    f.receive(wrong_revision, 2).await.unwrap();
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
        .all(|call| !call.path.ends_with("/callback")));
}

#[tokio::test]
async fn native_discord_multiselect_competing_clicks_cannot_replace_winning_set() {
    let f = FixtureState::new().await;
    let card = f.card(&multiselect_message()).await;
    let custom = card.markup[0]["components"][0]["custom_id"]
        .as_str()
        .unwrap();
    let mut first = f.callback(custom);
    first["data"]["component_type"] = 3.into();
    first["data"]["values"] = json!(["0"]);
    f.setup.fixture.advance(1);
    let mut second = f.callback(custom);
    second["data"]["component_type"] = 3.into();
    second["data"]["values"] = json!(["1"]);
    let dispatch = |data| super::Dispatch {
        name: "INTERACTION_CREATE".into(),
        sequence: 2,
        data,
    };
    let first_dispatch = dispatch(first.clone());
    let second_dispatch = dispatch(second.clone());
    // Exercise competing card CAS writes without the Gateway's serial lock.
    let journal = &f.setup.journal;
    let (first_result, second_result) = tokio::join!(
        journal.persist_component(&journal.authority, &first_dispatch),
        journal.persist_component(&journal.authority, &second_dispatch)
    );
    first_result.unwrap();
    second_result.unwrap();
    let saved = serde_json::to_value(f.store.load(&card.card.id).await.unwrap().0).unwrap();
    let (winner, expected) = if saved["consumed"]["callback_id"] == first["id"] {
        (first, json!(["private-read"]))
    } else {
        (second, json!(["private-write"]))
    };
    f.receive(winner.clone(), 3).await.unwrap();
    assert_eq!(selected(&f, &winner).await, expected);
    assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_discord_multiselect_stored_selection_is_required_and_bounded() {
    let f = FixtureState::new().await;
    let card = f.card(&multiselect_message()).await;
    let mut raw = f.callback(
        card.markup[0]["components"][0]["custom_id"]
            .as_str()
            .unwrap(),
    );
    raw["data"]["component_type"] = 3.into();
    raw["data"]["values"] = json!(["0", "1"]);
    f.receive(raw, 2).await.unwrap();
    let saved = serde_json::to_value(f.store.load(&card.card.id).await.unwrap().0).unwrap();
    for invalid in [Value::Null, json!([1, 0]), json!([0, 0]), json!([2])] {
        let mut value = saved.clone();
        value["consumed"]["selection"] = invalid;
        let card = serde_json::from_value(value).unwrap();
        assert!(!crate::interactions::discord::valid(&card));
    }
    let mut wrong_action = saved.clone();
    wrong_action["consumed"]["action"] = 1.into();
    assert!(!crate::interactions::discord::valid(
        &serde_json::from_value(wrong_action).unwrap()
    ));
    let mut single_with_selection = saved.clone();
    single_with_selection["message"]["blocks"][0]["multi"] = false.into();
    assert!(!crate::interactions::discord::valid(
        &serde_json::from_value(single_with_selection).unwrap()
    ));
    let mut historical = saved;
    historical["consumed"]
        .as_object_mut()
        .unwrap()
        .remove("selection");
    let mut card: crate::interactions::Card = serde_json::from_value(historical).unwrap();
    assert!(!crate::interactions::discord::valid(&card));
    if let Block::Choice { multi, .. } = &mut card.message.blocks[0] {
        *multi = false;
    }
    assert!(crate::interactions::discord::valid(&card));
}

#[tokio::test]
async fn native_discord_multiselect_source_rotation_and_consumer_rebinding_revoke_selection() {
    for source in [false, true] {
        let f = FixtureState::named_thread(11).await;
        let card = f.card(&multiselect_message()).await;
        f.rest.memory.calls.lock().unwrap().clear();
        let mut definition = f.setup.channel.to_blob_json();
        if source {
            definition["secrets"]["discord_bot_token"] = "rotated-selection-fixture".into();
        } else {
            definition["routing"]["consumers"][0]["provider"]["binding_id"] = "bnd_replaced".into();
        }
        f.setup
            .fixture
            .replace(&serde_json::from_value(definition).unwrap());
        let mut raw = f.callback(
            card.markup[0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        );
        raw["data"]["component_type"] = 3.into();
        raw["data"]["values"] = json!(["0", "1"]);
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
