use super::*;
use layer2_core::trigger_domain::{
    LaunchStepStatus, LaunchWorkState, LaunchWorkStep, ResolvedResource,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ready_observation_keeps_history_bounded_and_other_commits_validate_history() {
    let seed = seeded("ready_observation").await;
    let public = published(&seed, "ready_observation").await;
    let mut candidate = launch(&public, 'a', 'b');
    candidate
        .start(LaunchConfirmation {
            surface: LaunchConsentSurface::Landing,
            trust_confirmed_digest: None,
            acknowledgements: vec![],
            confirmed_at: NOW.into(),
        })
        .unwrap();
    candidate
        .transition(LaunchState::Ready, LATER.into())
        .unwrap();
    candidate.first_message = FirstMessageDelivery::Completed;
    for step in &mut candidate.steps {
        step.status = LaunchStepStatus::Completed;
        step.started_at.get_or_insert_with(|| LATER.into());
        step.completed_at.get_or_insert_with(|| LATER.into());
    }
    candidate.work = LaunchWorkState::new(LaunchWorkStep::Finish, LATER.into());
    let initial = admit_candidate(&seed, &public, candidate).await;
    let claimed = seed
        .store
        .claim_launch(&initial.id, "observer", LATER.into(), LATEST.into())
        .await
        .unwrap()
        .unwrap();
    let mut changed = claimed.clone();
    let events = changed
        .defer_worker_launch(
            "observer",
            claimed.work.lease.as_ref().unwrap().fence,
            LATEST.into(),
            LATER.into(),
        )
        .unwrap();
    assert!(events.is_empty());
    let commit = LaunchCommit {
        expected_revision: claimed.revision,
        launch: changed,
        event_at: LATER.into(),
        events,
    };
    let stored = seed.store.commit_launch(commit).await.unwrap();
    assert_eq!(stored.event_head, initial.event_head);
    assert_eq!(
        seed.store
            .launch_events(&stored.id, &stored.owner, 0, 200)
            .await
            .unwrap()
            .len(),
        1
    );

    seed.client
        .database(&seed.database)
        .collection::<Document>(LAUNCH_EVENTS)
        .delete_many(doc! { "launch_id": stored.id.as_str() })
        .await
        .unwrap();
    let mut completion = stored.clone();
    completion
        .transition(LaunchState::Completed, LATEST.into())
        .unwrap();
    assert!(matches!(
        seed.store
            .commit_launch(LaunchCommit {
                expected_revision: stored.revision,
                launch: completion,
                event_at: LATEST.into(),
                events: vec![]
            })
            .await,
        Err(LaunchStoreError::Corrupt(_))
    ));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn source_launch_is_oldest_and_scoped_to_owner_agent_and_trigger() {
    let seed = seeded("launch_agent_source").await;
    let public = published(&seed, "launch_agent_source").await;
    let other_trigger = published(&seed, "other_source").await;
    let id = layer2_core::AgentId("agt_prepared_source".into());
    let mut original = launch(&public, 'a', 'b');
    original.resolution = LaunchResolution {
        sandbox: ResolvedResource::Reused(layer2_core::SandboxId("sbx_source".into())),
        workspace: ResolvedResource::Reused(layer2_core::WorkspaceId("wks_source".into())),
        agent: ResolvedResource::Reused(id.clone()),
        choice_required: false,
    };
    let first = admit_candidate(&seed, &public, original.clone()).await;
    let mut next = launch_at(&other_trigger, 'c', 'd', LATER);
    next.resolution = original.resolution;
    let second = admit_candidate(&seed, &other_trigger, next).await;
    assert_eq!(
        seed.store
            .source_launch(&first.owner, &id, None)
            .await
            .unwrap()
            .unwrap()
            .id,
        first.id
    );
    assert_eq!(
        seed.store
            .source_launch(&first.owner, &id, Some(&other_trigger.trigger_id))
            .await
            .unwrap()
            .unwrap()
            .id,
        second.id
    );
    assert!(seed
        .store
        .source_launch(&SubjectId::new("stranger"), &id, None)
        .await
        .unwrap()
        .is_none());
    assert!(seed
        .store
        .source_launch(
            &first.owner,
            &layer2_core::AgentId("agt_absent".into()),
            None
        )
        .await
        .unwrap()
        .is_none());
}
