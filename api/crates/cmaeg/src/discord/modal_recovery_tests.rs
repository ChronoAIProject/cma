use super::{
    components_fixture::FixtureState,
    modal_fixture::{card, launch, restored, submit},
};
use serde_json::{json, Value};

#[tokio::test]
async fn discord_modal_unknown_type9_never_repeats_but_new_launch_or_genuine_submit_works() {
    for resubmit in [false, true] {
        let f = FixtureState::new().await;
        let card = card(&f).await;
        f.rest
            .respond(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({}));
        let (raw, custom) = launch(&f, &card, 2).await;
        f.receive(raw.clone(), 3).await.unwrap();
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
        assert!(f
            .store
            .load(&card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
        f.rest
            .respond(axum::http::StatusCode::NO_CONTENT, Value::Null);
        let active = if resubmit {
            custom
        } else {
            f.setup.fixture.advance(1);
            let (_, fresh) = launch(&f, &card, 4).await;
            f.receive(raw, 5).await.unwrap();
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 2);
            fresh
        };
        let good = submit(&f, &active);
        f.receive(good.clone(), 6).await.unwrap();
        assert_eq!(
            restored(&f, &good).await.request.conversation_id,
            f.original.conversation_id
        );
    }
}
#[tokio::test]
async fn discord_modal_consumed_submit_recovers_lost_inbox_after_deadline_without_second_ack() {
    for committed in [false, true] {
        let f = FixtureState::new().await;
        let card = card(&f).await;
        let (_, custom) = launch(&f, &card, 2).await;
        let raw = submit(&f, &custom);
        f.setup.fixture.fail(&f.id(&raw), committed);
        assert_eq!(f.receive(raw.clone(), 3).await.is_ok(), committed);
        assert!(f
            .store
            .load(&card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_some());
        let expected_calls = if committed { 2 } else { 1 };
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), expected_calls);
        f.setup.fixture.advance(3000);
        f.receive(raw.clone(), 3).await.unwrap();
        assert_eq!(
            restored(&f, &raw).await.request.conversation_id,
            f.original.conversation_id
        );
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), expected_calls);
        f.receive(raw, 4).await.unwrap();
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), expected_calls);
    }
}
#[tokio::test]
async fn discord_modal_late_fresh_launch_and_submission_never_consume() {
    let f = FixtureState::new().await;
    let card = card(&f).await;
    let raw = f.callback(
        card.markup[0]["components"][0]["custom_id"]
            .as_str()
            .unwrap(),
    );
    f.setup.fixture.advance(3000);
    f.receive(raw, 2).await.unwrap();
    assert!(f.rest.memory.calls.lock().unwrap().is_empty());
    let (_, custom) = launch(&f, &card, 3).await;
    let raw = submit(&f, &custom);
    f.setup.fixture.advance(3000);
    f.receive(raw, 4).await.unwrap();
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn discord_modal_changed_submission_cannot_repair_a_consumed_result() {
    let f = FixtureState::new().await;
    let card = card(&f).await;
    let (_, custom) = launch(&f, &card, 2).await;
    let mut raw = submit(&f, &custom);
    f.setup.fixture.fail(&f.id(&raw), false);
    assert!(f.receive(raw.clone(), 3).await.is_err());
    raw["data"]["components"][0]["component"]["value"] = "changed".into();
    f.receive(raw.clone(), 3).await.unwrap();
    assert!(!f
        .setup
        .fixture
        .memory
        .rows
        .lock()
        .unwrap()
        .contains_key(&f.id(&raw)));
    raw["data"]["components"][0]["component"]["value"] = "Ada".into();
    f.receive(raw.clone(), 4).await.unwrap();
    let result = restored(&f, &raw).await;
    assert!(serde_json::to_string(&result.request.activity)
        .unwrap()
        .contains("Ada"));
}

#[tokio::test]
async fn discord_modal_malformed_replay_cannot_poison_accepted_inbox_recovery() {
    for stale_rejection in [false, true] {
        for malformed in ["number", "shape"] {
            let f = FixtureState::new().await;
            let card = card(&f).await;
            let (_, custom) = launch(&f, &card, 2).await;
            let original = submit(&f, &custom);
            f.setup.fixture.fail(&f.id(&original), false);
            assert!(f.receive(original.clone(), 3).await.is_err());
            let before =
                serde_json::to_value(f.store.load(&card.card.id).await.unwrap().0).unwrap();
            assert!(before["consumed"].is_object());
            let id = original["id"].as_str().unwrap();
            let rejected = crate::connections::store::index_id(
                "discord-modal-rejected-v1",
                &[&f.setup.fixture.record, "77", id],
            );
            if stale_rejection {
                // A delayed invalid callback or older reader may already have
                // written a rejection. It cannot supersede accepted custody.
                f.setup
                    .fixture
                    .store
                    .put(
                        &rejected,
                        &f.setup.channel.owner.subject,
                        &json!({"kind":"discord_modal_rejected_v1","interaction":id}),
                        None,
                        Some(f.setup.now() + super::journal_model::RETENTION_SECONDS),
                    )
                    .await
                    .unwrap();
            }
            let mut changed = original.clone();
            if malformed == "number" {
                changed["data"]["components"][2]["component"]["value"] = "NaN".into();
            } else {
                changed["data"]["components"][0]["component"]["custom_id"] = "unknown".into();
            }
            // No clock advance: the changed replay is inside the initial ACK
            // budget, where incorrectly classifying it as new would poison it.
            f.receive(changed, 3).await.unwrap();
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
            assert_eq!(
                f.setup
                    .fixture
                    .memory
                    .rows
                    .lock()
                    .unwrap()
                    .contains_key(&rejected),
                stale_rejection
            );
            let after = serde_json::to_value(f.store.load(&card.card.id).await.unwrap().0).unwrap();
            assert_eq!(after["consumed"], before["consumed"]);
            f.receive(original.clone(), 4).await.unwrap();
            let accepted = restored(&f, &original).await;
            let cmaeg_core::activity::ActivityKind::Interaction(result) =
                accepted.request.activity.kind
            else {
                panic!()
            };
            assert_eq!(result.values["fields"], before["consumed"]["submitted"]);
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 2);
            f.receive(original, 5).await.unwrap();
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 2);
        }
    }
}
