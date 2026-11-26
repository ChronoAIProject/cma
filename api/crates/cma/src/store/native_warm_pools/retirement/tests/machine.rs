use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn migrated_desktop_selection_retires_only_the_previous_size_on_the_same_image() {
    let (db, store, mut old, _) = fixture().await;
    old.id = "old-desktop-size".into();
    old.machine = true;
    old.document_schema_version = 2;
    old.plan.config_sha256 = "c".repeat(64);
    old.plan.spec.image.uri = format!("registry.example/desktop@sha256:{}", "c".repeat(64));
    old.target_replicas = 2;
    old.binding = None;
    store.insert(old.clone()).await.unwrap();

    let mut selected = old.clone();
    selected.id = "new-desktop-size".into();
    selected.plan.config_sha256 = "d".repeat(64);
    selected.plan.spec.memory_mib *= 2;
    store.insert(selected.clone()).await.unwrap();
    let image = selected.plan.spec.image.uri.clone();
    assert!(store
        .retirement_drain_homes(&old, &image)
        .await
        .unwrap()
        .is_none());

    // Both legacy records inherited the image's activation time. The exact
    // configured default is selected once and gets an independent settle window.
    store.activate(&selected.id).await.unwrap();
    let collection = db.collection::<Document>(COLLECTION);
    collection
        .update_one(
            doc! {"_id": &selected.id},
            doc! {"$set": {
                "machine_plan_activated_at": DateTime::from_millis(2),
            }},
        )
        .await
        .unwrap();
    let selected = store.find(&selected.id).await.unwrap().unwrap();
    assert_eq!(selected.activated_at, old.activated_at);
    assert!(matches!(
        store.activate(&old.id).await,
        Err(WarmPoolError::Conflict)
    ));
    assert_eq!(
        store.retirement_drain_homes(&old, &image).await.unwrap(),
        drain_homes_only(vec![])
    );
    assert!(store
        .retirement_drain_homes(&selected, &image)
        .await
        .unwrap()
        .is_none());

    let retired = store.begin_retirement(&old).await.unwrap();
    assert_eq!(retired.target_replicas, 0);
    assert!(retired.retiring);
    assert_eq!(
        store
            .retirement_drain_homes(&retired, &image)
            .await
            .unwrap(),
        drain_homes_only(vec![])
    );
    let restarted = MongoNativeWarmPools::open(&db, "test").await.unwrap();
    restarted.activate(&selected.id).await.unwrap();
    assert!(matches!(
        restarted.activate(&old.id).await,
        Err(WarmPoolError::Conflict)
    ));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn machine_and_agent_release_order_remain_independent_across_restart() {
    let (db, store, old_agent, agent_image) = fixture().await;
    let mut machine = old_agent.clone();
    machine.id = "machine-first".into();
    machine.machine = true;
    machine.document_schema_version = 2;
    machine.plan.config_sha256 = "c".repeat(64);
    machine.plan.spec.image.uri = format!("registry.example/omarchy@sha256:{}", "c".repeat(64));
    machine.activated_at = Some(DateTime::from_millis(3));
    machine.binding = None;
    machine.lease = None;
    machine.lease_until = DateTime::from_millis(0);
    store.insert(machine.clone()).await.unwrap();
    store.activate(&machine.id).await.unwrap();
    store.activate("current").await.unwrap();
    assert!(store
        .retirement_homes(&old_agent, &agent_image)
        .await
        .unwrap()
        .is_some());
    assert!(store
        .retirement_homes(&old_agent, &machine.plan.spec.image.uri)
        .await
        .unwrap()
        .is_none());
    let reopened = MongoNativeWarmPools::open(&db, "test").await.unwrap();
    assert_eq!(reopened.list().await.unwrap().len(), 3);
    let claimed = reopened.claim_target(&machine.id).await.unwrap().unwrap();
    assert!(claimed.machine);
    assert_eq!(claimed.document_schema_version, 2);
    reopened.finish(&claimed, true).await.unwrap();
    let mut next = machine.clone();
    next.id = "machine-second".into();
    next.plan.config_sha256 = "d".repeat(64);
    next.plan.spec.image.uri = format!("registry.example/omarchy@sha256:{}", "d".repeat(64));
    next.activated_at = Some(DateTime::from_millis(4));
    reopened.insert(next.clone()).await.unwrap();
    reopened.activate(&next.id).await.unwrap();
    reopened.activate("current").await.unwrap();
    assert!(matches!(
        reopened.activate(&machine.id).await,
        Err(WarmPoolError::Conflict)
    ));
    // The previous machine release cannot acquire a fresh release timestamp
    // by changing its resources or immutable Pod policy.
    let mut stale = machine.clone();
    stale.id = "machine-old-new-policy".into();
    stale.plan.config_sha256 = "e".repeat(64);
    stale.activated_at = None;
    reopened.insert(stale.clone()).await.unwrap();
    assert!(matches!(
        reopened.activate(&stale.id).await,
        Err(WarmPoolError::Conflict)
    ));
    assert_eq!(
        reopened
            .find(&stale.id)
            .await
            .unwrap()
            .unwrap()
            .activated_at,
        machine.activated_at
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn pool_schema_rejects_ambiguous_machine_channel_and_legacy_reader() {
    let (db, store, mut machine, _) = fixture().await;
    machine.id = "machine".into();
    machine.machine = true;
    machine.document_schema_version = 2;
    machine.plan.config_sha256 = "c".repeat(64);
    machine.binding = None;
    store.insert(machine).await.unwrap();
    assert!(
        crate::store::schema::refuse_unknown_versions(&db, &[COLLECTION])
            .await
            .is_err()
    );
    let collection = db.collection::<Document>(COLLECTION);
    for value in [
        mongodb::bson::Bson::Boolean(false),
        mongodb::bson::Bson::Null,
        "true".into(),
        vec![true].into(),
    ] {
        collection
            .update_one(doc! {"_id":"machine"}, doc! {"$set":{"machine":value}})
            .await
            .unwrap();
        assert!(MongoNativeWarmPools::open(&db, "test").await.is_err());
    }
    db.drop().await.unwrap();
}
