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
