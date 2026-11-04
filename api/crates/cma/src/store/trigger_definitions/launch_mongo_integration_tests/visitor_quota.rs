use super::*;

fn visitor(trigger: &PublicTrigger, key: char) -> LaunchAdmission {
    let mut candidate = launch(trigger, key, key);
    candidate.authority = LaunchAuthorityClass::Visitor;
    LaunchAdmission {
        launch: candidate,
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn visitor_admission_is_atomic_across_replicas_and_replays_after_restart() {
    let seed = seeded("visitor_quota").await;
    let trigger = published(&seed, "visitor_quota").await;
    let database = seed.client.database(&seed.database);
    let settings_store =
        crate::store::MongoPlatformSettingsStore::open(&seed.client, &seed.database)
            .await
            .unwrap();
    use layer2_core::PlatformSettingsStore;
    let mut settings = layer2_core::PlatformSettings::default();
    settings.limits.visitor_max_launches_per_day = 1;
    settings_store.replace(0, &settings).await.unwrap();
    let other =
        MongoTriggerDefinitionStore::open(&seed.client, &seed.database, seed.profiles.clone())
            .await
            .unwrap();
    let a = visitor(&trigger, 'a');
    let b = visitor(&trigger, 'b');
    let (left, right) = tokio::join!(
        seed.store.admit_launch(a.clone()),
        other.admit_launch(b.clone())
    );
    let winner = match (left, right) {
        (
            Ok(LaunchAdmissionResult::Created(_)),
            Err(LaunchStoreError::VisitorQuota { limit: 1, .. }),
        ) => a,
        (
            Err(LaunchStoreError::VisitorQuota { limit: 1, .. }),
            Ok(LaunchAdmissionResult::Created(_)),
        ) => b,
        result => panic!("one replica must consume the allowance: {result:?}"),
    };
    let restarted =
        MongoTriggerDefinitionStore::open(&seed.client, &seed.database, seed.profiles.clone())
            .await
            .unwrap();
    assert!(matches!(
        restarted.admit_launch(winner).await.unwrap(),
        LaunchAdmissionResult::Existing(_)
    ));
    assert!(matches!(
        restarted.admit_launch(visitor(&trigger, 'c')).await,
        Err(LaunchStoreError::VisitorQuota { .. })
    ));
    assert_eq!(
        database
            .collection::<Document>(LAUNCHES)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    settings.limits.visitor_max_launches_per_day = 2;
    settings_store.replace(1, &settings).await.unwrap();
    // Simulate the first shared-quota rollout with only historical Launches present.
    database
        .collection::<Document>("visitor_launch_days")
        .delete_many(doc! {})
        .await
        .unwrap();
    assert!(matches!(
        restarted
            .admit_launch(visitor(&trigger, 'c'))
            .await
            .unwrap(),
        LaunchAdmissionResult::Created(_)
    ));
    assert!(matches!(
        restarted.admit_launch(visitor(&trigger, 'd')).await,
        Err(LaunchStoreError::VisitorQuota { limit: 2, .. })
    ));
    let mut tomorrow = visitor(&trigger, 'e');
    tomorrow.launch = launch_at(&trigger, 'e', 'e', "2026-09-06T00:00:00Z");
    tomorrow.launch.authority = LaunchAuthorityClass::Visitor;
    assert!(matches!(
        restarted.admit_launch(tomorrow).await.unwrap(),
        LaunchAdmissionResult::Created(_)
    ));
    database.drop().await.unwrap();
}
