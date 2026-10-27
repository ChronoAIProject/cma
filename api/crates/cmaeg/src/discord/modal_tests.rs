use super::{
    components_fixture::FixtureState,
    modal_fixture::{card, launch, restored, submit},
};
use cmaeg_core::activity::ActivityKind;
use serde_json::{json, Value};

#[tokio::test]
async fn discord_modal_launch_is_nonconsuming_and_submission_routes_one_original_consumer() {
    let f = FixtureState::named().await;
    f.settle_context_message().await;
    let card = card(&f).await;
    let (clicked, custom) = launch(&f, &card, 2).await;
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    assert!(!f
        .setup
        .fixture
        .memory
        .rows
        .lock()
        .unwrap()
        .contains_key(&f.id(&clicked)));
    assert!(f.provider.memory.lock().unwrap().seen.is_empty());
    {
        let calls = f.rest.memory.calls.lock().unwrap();
        let call = calls.last().unwrap();
        assert!(call.authorization.is_none());
        let body: Value = serde_json::from_slice(&call.body).unwrap();
        assert_eq!(body["type"], 9);
        assert_eq!(body["data"]["components"][0]["type"], 18);
        assert_eq!(body["data"]["components"][1]["component"]["min_values"], 0);
        assert_eq!(
            body["data"]["components"][2]["component"]["placeholder"],
            "Number (e.g. 12.5)"
        );
        assert_eq!(
            body["data"]["components"][3]["component"]["placeholder"],
            "Date (YYYY-MM-DD)"
        );
        assert!(!body.to_string().contains("private-"));
        assert!(body["data"]["custom_id"].as_str().unwrap().len() <= 100);
    }
    let mut raw = submit(&f, &custom);
    raw.as_object_mut().unwrap().remove("message");
    let records = f.setup.fixture.memory.clone();
    let expected = f.id(&raw);
    *f.rest.memory.on_call.lock().unwrap() = Some(std::sync::Arc::new(move |_| {
        assert!(records.rows.lock().unwrap().contains_key(&expected));
    }));
    f.receive(raw.clone(), 3).await.unwrap();
    let saved = restored(&f, &raw).await;
    assert_eq!(saved.request.consumer, f.original.consumer);
    assert_eq!(saved.request.conversation_id, f.original.conversation_id);
    let ActivityKind::Interaction(result) = &saved.request.activity.kind else {
        panic!()
    };
    assert_eq!(result.block_id, "private-form");
    assert_eq!(result.action_id, "private-form");
    assert_eq!(
        result.values["fields"],
        json!({"private-text":"Ada","private-choice":"private-write","private-number":12.5,"private-date":"2028-02-29"})
    );
    assert!(!String::from_utf8_lossy(&saved.source_bytes).contains("private-interaction-token"));
    let calls = f.rest.memory.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        serde_json::from_slice::<Value>(&calls[1].body).unwrap(),
        json!({"type":6})
    );
    f.receive(raw.clone(), 4).await.unwrap();
    assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 2);
    assert_eq!(f.setup.sweep(f.setup.now()).await.pending, 0);
    let plan = f
        .setup
        .plans
        .replay_native_interaction(&saved.request, &saved.source_bytes)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(
        plan.targets[0].launch.consumer,
        f.original.consumer.clone().unwrap()
    );
    assert!(f
        .setup
        .plans
        .replay(&saved.request, &saved.source_bytes)
        .await
        .is_err());
}
#[tokio::test]
async fn discord_modal_invalid_values_do_not_consume_and_fresh_launch_can_retry() {
    let f = FixtureState::new().await;
    let card = card(&f).await;
    let (_, custom) = launch(&f, &card, 2).await;
    let mut bad = submit(&f, &custom);
    bad["data"]["components"][2]["component"]["value"] = "not-a-number".into();
    f.receive(bad.clone(), 3).await.unwrap();
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    assert!(!f
        .setup
        .fixture
        .memory
        .rows
        .lock()
        .unwrap()
        .contains_key(&f.id(&bad)));
    let body: Value =
        serde_json::from_slice(&f.rest.memory.calls.lock().unwrap().last().unwrap().body).unwrap();
    assert_eq!(
        body,
        json!({"type":4,"data":{"content":"Please check the form values and open the form again to retry.","flags":64,"allowed_mentions":{"parse":[]}}})
    );
    bad["data"]["components"] = super::modal_fixture::values();
    f.receive(bad, 4).await.unwrap();
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    f.setup.fixture.advance(1);
    let (_, fresh) = launch(&f, &card, 5).await;
    assert_ne!(fresh, custom);
    let old = submit(&f, &custom);
    f.receive(old, 6).await.unwrap();
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    let good = submit(&f, &fresh);
    f.receive(good.clone(), 7).await.unwrap();
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_some());
    assert_eq!(
        restored(&f, &good).await.request.conversation_id,
        f.original.conversation_id
    );
}
#[tokio::test]
async fn discord_modal_identity_and_current_provider_are_required_at_both_stages() {
    for submit_stage in [false, true] {
        for (pointer, value) in [
            ("/application_id", json!("78")),
            ("/user/id", json!("43")),
            ("/channel_id", json!("100")),
            ("/message/id", json!("801")),
            ("/message/author/id", json!("78")),
            ("/channel/type", json!(11)),
            ("/token", json!("bad/token")),
        ] {
            let f = FixtureState::new().await;
            let card = card(&f).await;
            let mut raw = if submit_stage {
                let (_, custom) = launch(&f, &card, 2).await;
                submit(&f, &custom)
            } else {
                f.callback(
                    card.markup[0]["components"][0]["custom_id"]
                        .as_str()
                        .unwrap(),
                )
            };
            let count = f.rest.memory.calls.lock().unwrap().len();
            *raw.pointer_mut(pointer).unwrap() = value;
            f.receive(raw, 3).await.unwrap();
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
            assert_eq!(
                f.rest.memory.calls.lock().unwrap().len(),
                count,
                "{pointer}"
            );
        }
        for source in [false, true] {
            let f = FixtureState::named().await;
            let card = card(&f).await;
            let raw = if submit_stage {
                let (_, custom) = launch(&f, &card, 2).await;
                submit(&f, &custom)
            } else {
                f.callback(
                    card.markup[0]["components"][0]["custom_id"]
                        .as_str()
                        .unwrap(),
                )
            };
            let count = f.rest.memory.calls.lock().unwrap().len();
            let mut definition = f.setup.channel.to_blob_json();
            if source {
                definition["secrets"]["discord_bot_token"] = "changed-token".into();
            } else {
                definition["routing"]["consumers"][0]["provider"]["binding_id"] =
                    "bnd_changed".into();
            }
            f.setup
                .fixture
                .replace(&serde_json::from_value(definition).unwrap());
            let result = f.receive(raw, 3).await;
            if source {
                assert!(result.is_err())
            } else {
                result.unwrap()
            };
            assert!(f
                .store
                .load(&card.card.id)
                .await
                .unwrap()
                .0
                .consumed
                .is_none());
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), count);
        }
    }
}
#[tokio::test]
async fn discord_modal_existing_thread_and_guild_are_retained() {
    for kind in [10, 11, 12] {
        let f = FixtureState::named_thread(kind).await;
        let card = card(&f).await;
        let (_, custom) = launch(&f, &card, 2).await;
        let mut wrong = submit(&f, &custom);
        wrong["guild_id"] = "89".into();
        f.receive(wrong, 3).await.unwrap();
        assert!(f
            .store
            .load(&card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
        let good = submit(&f, &custom);
        f.queue_thread_metadata();
        f.receive(good.clone(), 4).await.unwrap();
        let saved = restored(&f, &good).await;
        assert_eq!(
            saved.request.activity.conversation,
            f.context.activity.conversation
        );
        assert_eq!(saved.request.consumer, f.original.consumer);
    }
}
