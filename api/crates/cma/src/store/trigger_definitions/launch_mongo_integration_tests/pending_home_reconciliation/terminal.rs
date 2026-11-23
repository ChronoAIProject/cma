use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn completed_and_failed_terminal_leaks_are_repaired_while_retryable_failure_is_retained() {
    use layer2_core::trigger_domain::{LaunchFailure, LaunchStepStatus};
    let (seed, public, reconciler) = setup("pending_terminal_states").await;
    let mut launches = Vec::new();
    for (index, state) in [
        LaunchState::Completed,
        LaunchState::FailedTerminal,
        LaunchState::FailedRetryable,
    ]
    .into_iter()
    .enumerate()
    {
        let mut current = seed
            .store
            .admit_launch(request(&public, index))
            .await
            .unwrap()
            .launch()
            .clone();
        seed.client
            .database(&seed.database)
            .collection::<Document>(LAUNCHES)
            .update_one(
                doc! {"_id":current.id.as_str()},
                doc! {"$unset":{"home_capacity_admission":""}},
            )
            .await
            .unwrap();
        let revision = current.revision;
        current
            .start(LaunchConfirmation {
                surface: LaunchConsentSurface::Landing,
                trust_confirmed_digest: None,
                acknowledgements: vec![],
                confirmed_at: NOW.into(),
            })
            .unwrap();
        current = seed
            .store
            .commit_launch(LaunchCommit {
                expected_revision: revision,
                launch: current,
                event_at: NOW.into(),
                events: vec![
                    LaunchEventKind::ConfirmationRecorded,
                    LaunchEventKind::StateChanged {
                        from: LaunchState::Draft,
                        to: LaunchState::Resolving,
                    },
                ],
            })
            .await
            .unwrap();
        let revision = current.revision;
        if state == LaunchState::Completed {
            current.state = LaunchState::Ready;
            current.ready_at = Some(LATER.into());
            current.updated_at = LATER.into();
            current.revision += 1;
            current.first_message = FirstMessageDelivery::Completed;
            for step in &mut current.steps {
                step.status = LaunchStepStatus::Completed;
                step.started_at.get_or_insert_with(|| LATER.into());
                step.completed_at.get_or_insert_with(|| LATER.into());
            }
            current.work = layer2_core::trigger_domain::LaunchWorkState::new(
                layer2_core::trigger_domain::LaunchWorkStep::Finish,
                LATER.into(),
            );
            current = seed
                .store
                .commit_launch(LaunchCommit {
                    expected_revision: revision,
                    launch: current,
                    event_at: LATER.into(),
                    events: vec![],
                })
                .await
                .unwrap();
            let revision = current.revision;
            current
                .transition(LaunchState::Completed, LATEST.into())
                .unwrap();
            current = seed
                .store
                .commit_launch(LaunchCommit {
                    expected_revision: revision,
                    launch: current,
                    event_at: LATEST.into(),
                    events: vec![LaunchEventKind::Completed],
                })
                .await
                .unwrap();
        } else {
            let failure = LaunchFailure {
                code: "fixture_capacity_failure".into(),
                safe_detail: "A fixture resource step failed.".into(),
                retryable: state == LaunchState::FailedRetryable,
                occurred_at: LATER.into(),
            };
            current.failure = Some(failure.clone());
            current.transition(state, LATER.into()).unwrap();
            current = seed
                .store
                .commit_launch(LaunchCommit {
                    expected_revision: revision,
                    launch: current,
                    event_at: LATER.into(),
                    events: vec![
                        LaunchEventKind::StateChanged {
                            from: LaunchState::Resolving,
                            to: state,
                        },
                        LaunchEventKind::Failed { failure },
                    ],
                })
                .await
                .unwrap();
        }
        launches.push(current);
    }
    assert_eq!(reconciler.sweep(None).await.unwrap().released, 2);
    for (index, launch) in launches.iter().enumerate() {
        assert_eq!(
            receipt(&seed, launch).await.get_str("state").unwrap(),
            if index < 2 { "released" } else { "reserved" }
        );
    }
    assert_eq!(reconciler.observe().await.unwrap().pending, 1);
    seed.client.database(&seed.database).drop().await.unwrap();
}
