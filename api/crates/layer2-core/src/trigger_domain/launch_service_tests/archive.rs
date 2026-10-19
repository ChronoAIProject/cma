use super::*;

#[tokio::test]
async fn archived_trigger_preserves_admitted_visitor_drafts_and_rejects_new_launches() {
    for context in [ExistingContext::Nothing, ExistingContext::LiveAgent] {
        let fixture = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
        fixture.set_context(context);
        let draft = fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("before-archive"),
            )
            .await
            .unwrap()
            .launch;
        fixture
            .definitions
            .delete_trigger(
                &fixture.public.trigger_id,
                &fixture.public.owner,
                "2026-10-19T02:00:00Z".into(),
            )
            .await
            .unwrap();
        assert!(fixture
            .definitions
            .trigger(&fixture.public.trigger_id, &fixture.public.owner)
            .await
            .unwrap()
            .is_none());
        let started = fixture
            .service
            .start(
                &fixture.caller,
                &draft.id,
                StartLaunch {
                    trust_confirmed_digest: None,
                    acknowledgements: vec![],
                },
            )
            .await
            .unwrap();
        assert_eq!(started.state, LaunchState::Resolving);
        assert_eq!(started.owner, draft.owner);
        assert_eq!(started.revision_digest, draft.revision_digest);
        assert_eq!(
            fixture
                .service
                .create_or_resume(
                    &fixture.caller,
                    &fixture.public.public_id,
                    fixture.request("after-archive"),
                )
                .await
                .unwrap_err(),
            LaunchServiceError::TriggerRetired
        );
    }
}
