use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn disabled_and_enabled_replicas_preserve_quota_replay_and_cancellation_across_days() {
    for policy in [
        LaunchAdmissionPolicy::Baseline,
        LaunchAdmissionPolicy::VisitorAllowance,
    ] {
        let seed = seeded("visitor_rollout").await;
        let trigger = published(&seed, "rollout").await;
        let stores = replicas(&seed)
            .await
            .map(|store| store.with_launch_admission_policy(policy));
        let day = layer2_core::seconds_from_rfc3339(BEFORE_MIDNIGHT).unwrap() / 86_400;
        assert_eq!(
            stores[0]
                .visitor_launches_admitted(&SubjectId::new("visitor"), day)
                .await,
            Ok(0)
        );
        let original = admission(&trigger, "visitor", 0, BEFORE_MIDNIGHT);
        for key in 0..5 {
            assert!(stores[key % 2]
                .admit_launch(admission(&trigger, "visitor", key, BEFORE_MIDNIGHT))
                .await
                .unwrap()
                .was_created());
        }
        assert!(matches!(
            stores[0]
                .admit_launch(admission(&trigger, "visitor", 5, BEFORE_MIDNIGHT))
                .await,
            Err(LaunchStoreError::VisitorQuota { limit: 5, .. })
        ));
        let mut replay = original.clone();
        replay.launch.created_at = AFTER_MIDNIGHT.into();
        replay.launch.updated_at = AFTER_MIDNIGHT.into();
        let existing = stores[1].admit_launch(replay.clone()).await.unwrap();
        assert!(!existing.was_created());
        assert_eq!(existing.launch().created_at, BEFORE_MIDNIGHT);
        replay.launch.command_fingerprint = digest('c');
        assert_eq!(
            stores[0].admit_launch(replay).await,
            Err(LaunchStoreError::CommandConflict)
        );
        let mut cancelled = existing.launch().clone();
        let revision = cancelled.revision;
        cancelled.cancel(AFTER_MIDNIGHT.into()).unwrap();
        stores[1]
            .commit_launch(LaunchCommit {
                expected_revision: revision,
                launch: cancelled,
                event_at: AFTER_MIDNIGHT.into(),
                events: vec![LaunchEventKind::Cancelled],
            })
            .await
            .unwrap();
        assert_eq!(used(&seed, "visitor", BEFORE_MIDNIGHT).await, 5);
        assert_eq!(
            stores[1]
                .visitor_launches_admitted(&SubjectId::new("visitor"), day + 1)
                .await,
            Ok(0)
        );
        for key in 5..10 {
            assert!(stores[key % 2]
                .admit_launch(admission(&trigger, "visitor", key, AFTER_MIDNIGHT))
                .await
                .unwrap()
                .was_created());
        }
        assert_eq!(used(&seed, "visitor", AFTER_MIDNIGHT).await, 5);
        assert_eq!(stats(&seed, &trigger).await.launches_total, 10);
        seed.client.database(&seed.database).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn disabled_contention_retains_baseline_refusals_without_overspending() {
    let seed = seeded("visitor_baseline_fifty").await;
    let triggers = [
        published(&seed, "baseline_first").await,
        published(&seed, "baseline_second").await,
    ];
    let stores = replicas(&seed)
        .await
        .map(|store| store.with_launch_admission_policy(LaunchAdmissionPolicy::Baseline));
    let candidates: Vec<_> = (0..50)
        .map(|key| admission(&triggers[key % 2], "visitor", key, BEFORE_MIDNIGHT))
        .collect();
    let results = race(&stores, candidates.clone()).await;
    for result in &results {
        if let Err(LaunchStoreError::Unavailable(reason)) = result {
            assert!(
                reason.starts_with("Launch admission exhausted its retry budget:"),
                "baseline contention must not hide another failure: {reason}"
            );
        }
    }
    let (created, existing, refused, unavailable) = outcomes(&results);
    assert_eq!(existing, 0);
    assert!(created <= 5);
    assert_eq!(created + refused + unavailable, 50);
    assert_eq!(
        used(&seed, "visitor", BEFORE_MIDNIGHT).await,
        created as i64
    );
    for candidate in candidates {
        match stores[0].admit_launch(candidate).await {
            Ok(LaunchAdmissionResult::Created(_) | LaunchAdmissionResult::Existing(_))
            | Err(LaunchStoreError::VisitorQuota { limit: 5, .. }) => {}
            other => panic!("sequential recovery must settle explicitly: {other:?}"),
        }
    }
    assert_eq!(used(&seed, "visitor", BEFORE_MIDNIGHT).await, 5);
    seed.client.database(&seed.database).drop().await.unwrap();
}
