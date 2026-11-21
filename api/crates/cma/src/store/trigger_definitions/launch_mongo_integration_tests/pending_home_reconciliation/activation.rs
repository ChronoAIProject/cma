use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn callback_activation_and_new_startup_observations_enable_the_same_reconciler() {
    use crate::auth::registration::RegistrationCapacity;
    let (client, commands) = monitored_client().await;
    let name = format!("cma_pending_activation_{}", uuid::Uuid::new_v4().simple());
    let registration = crate::store::MongoRegistrationCapacityStore::open(&client, &name, false)
        .await
        .unwrap();
    let pending = enabled(&client, &name)
        .await
        .unwrap()
        .with_activation(registration.activation_observation());
    commands.lock().unwrap().clear();
    assert!(!pending.active());
    pending.sweep(None).await.unwrap();
    assert!(commands.lock().unwrap().is_empty());
    crate::store::MongoRegistrationCapacityStore::open(&client, &name, true)
        .await
        .unwrap();
    assert!(!pending.active());
    assert!(registration.tracking_active().await.unwrap());
    assert!(pending.active());
    prepare_index(&client, &name).await;
    assert_eq!(pending.observe().await.unwrap().drift, 0);
    client.database(&name).drop().await.unwrap();

    let name = format!(
        "cma_pending_startup_activation_{}",
        uuid::Uuid::new_v4().simple()
    );
    crate::store::MongoRegistrationCapacityStore::open(&client, &name, true)
        .await
        .unwrap();
    let restarted = crate::store::MongoRegistrationCapacityStore::open(&client, &name, false)
        .await
        .unwrap();
    let pending = PendingLaunchHomes::new(&client, &name)
        .with_enabled(true)
        .with_activation(restarted.activation_observation());
    assert!(
        pending.active(),
        "startup injects the persisted activation observation"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn repairing_the_same_receipt_as_a_normal_terminal_commit_decrements_once() {
    let (seed, public, reconciler) = setup("pending_same_release").await;
    let mut terminal = leak(&seed, &public, 0).await;
    seed.client
        .database(&seed.database)
        .collection::<Document>(LAUNCHES)
        .update_one(
            doc! {"_id":terminal.id.as_str()},
            doc! {"$set":{"home_capacity_admission":true}},
        )
        .await
        .unwrap();
    let expected_revision = terminal.revision;
    terminal.revision += 1;
    terminal.updated_at = LATEST.into();
    let other =
        MongoTriggerDefinitionStore::open(&seed.client, &seed.database, seed.profiles.clone())
            .await
            .unwrap();
    let (repair, normal) = tokio::join!(
        reconciler.sweep(None),
        other.commit_launch(LaunchCommit {
            expected_revision,
            launch: terminal.clone(),
            event_at: LATEST.into(),
            events: Vec::new()
        })
    );
    assert!(repair.unwrap().released <= 1);
    normal.unwrap();
    assert_eq!(pending(&seed).await, 0);
    assert_eq!(
        receipt(&seed, &terminal).await.get_str("state").unwrap(),
        "released"
    );
    assert_eq!(reconciler.observe().await.unwrap().drift, 0);
    seed.client.database(&seed.database).drop().await.unwrap();
}
