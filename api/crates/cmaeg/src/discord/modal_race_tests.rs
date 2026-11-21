use super::{
    components_fixture::FixtureState,
    modal_fixture::{card, launch, restored, submit},
};
use crate::interactions::{discord_edit::Admission, discord_form_store::Callback};
use std::time::Duration;

#[tokio::test]
async fn discord_modal_lost_dispatch_claim_never_retries_type9() {
    let f = FixtureState::new().await;
    let card = card(&f).await;
    let custom = card.markup[0]["components"][0]["custom_id"]
        .as_str()
        .unwrap();
    let raw = f.callback(custom);
    let id = raw["id"].as_str().unwrap();
    let created = (id.parse::<u64>().unwrap() >> 22) as i64 + 1_420_070_400_000;
    let (activity, _) = super::components::parse(&raw, "77", created).unwrap();
    f.store
        .launch_discord_form(
            &f.channel,
            &activity,
            &Callback {
                interaction: id,
                custom,
                message: "800",
                guild: None,
                thread: None,
            },
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await
        .unwrap();
    f.setup.fixture.fail(&card.card.id, true);
    assert!(f.receive(raw.clone(), 2).await.is_err());
    assert!(
        f.store
            .load(&card.card.id)
            .await
            .unwrap()
            .0
            .discord_form_launch
            .unwrap()
            .dispatched
    );
    assert!(f.rest.memory.calls.lock().unwrap().is_empty());
    f.receive(raw, 2).await.unwrap();
    assert!(f.rest.memory.calls.lock().unwrap().is_empty());
    f.setup.fixture.advance(1);
    let (_, fresh) = launch(&f, &card, 3).await;
    f.receive(submit(&f, &fresh), 4).await.unwrap();
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_some());
}
#[tokio::test]
async fn discord_modal_edit_delete_fences_fresh_submission_but_preserves_accepted_recovery() {
    for edit in [false, true] {
        for consumed in [false, true] {
            let f = FixtureState::new().await;
            let original = card(&f).await;
            let (_, custom) = launch(&f, &original, 2).await;
            let raw = submit(&f, &custom);
            if consumed {
                f.setup.fixture.fail(&f.id(&raw), false);
                assert!(f.receive(raw.clone(), 3).await.is_err());
            }
            let reference = original
                .card
                .reference
                .as_ref()
                .unwrap()
                .open(&f.keys())
                .unwrap()
                .payload;
            if edit {
                let mut changed = super::modal_fixture::message();
                changed.text = Some("Revised form".into());
                let Admission::Ready(candidate) = f
                    .store
                    .discord_edit_candidate(&f.channel, &reference, &changed)
                    .await
                    .unwrap()
                else {
                    panic!()
                };
                let mut claim = f
                    .store
                    .begin_discord_edit(*candidate, &changed)
                    .await
                    .unwrap();
                assert!(claim.prepared.card.discord_form_launch.is_none());
                assert!(claim.prepared.card.consumed.is_none());
                assert_eq!(claim.prepared.card.revision, 2);
                assert!(claim.prepared.markup[0]["components"][0]["custom_id"]
                    .as_str()
                    .unwrap()
                    .starts_with("cdf1:"));
                let destination = claim.prepared.card.destination.clone().unwrap();
                f.store
                    .bind(&mut claim.prepared, destination)
                    .await
                    .unwrap();
            } else {
                let candidate = f
                    .store
                    .discord_card_candidate(&f.channel, &reference)
                    .await
                    .unwrap();
                f.store
                    .begin_discord_effect(candidate, Some(&uuid::Uuid::new_v4().to_string()), true)
                    .await
                    .unwrap();
            }
            f.receive(raw.clone(), 4).await.unwrap();
            let retained = f.store.load(&original.card.id).await.unwrap().0;
            assert!(retained.superseded);
            assert_eq!(retained.consumed.is_some(), consumed);
            if consumed {
                assert_eq!(
                    restored(&f, &raw).await.request.conversation_id,
                    f.original.conversation_id
                );
            } else {
                assert!(!f
                    .setup
                    .fixture
                    .memory
                    .rows
                    .lock()
                    .unwrap()
                    .contains_key(&f.id(&raw)));
            }
        }
    }
}
