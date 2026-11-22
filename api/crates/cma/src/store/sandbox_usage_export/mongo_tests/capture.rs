use super::*;
use crate::store::sandbox_mongo::start_transaction;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn absent_capture_performs_no_new_namespace_io_or_ddl() {
    let fixture = Fixture::new(false).await;
    let parent = fixture.parent("no-wallet").await;
    fixture
        .store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    let store = fixture
        .store
        .clone()
        .with_usage_capture(None)
        .await
        .unwrap();
    store
        .checkpoint_usage_one(parent.id.as_str(), TWO_MINUTES)
        .await
        .unwrap();
    let trace = fixture.commands.lock().unwrap().clone();
    for command in trace {
        let text = command.to_string();
        assert!(
            !COLLECTIONS.iter().any(|name| text.contains(name)),
            "{text}"
        );
    }
    assert!(!fixture
        .db
        .list_collection_names()
        .await
        .unwrap()
        .iter()
        .any(|name| COLLECTIONS.contains(&name.as_str())));
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn enabled_capture_refuses_unprepared_or_ttl_schema_without_repair() {
    let fixture = Fixture::new(false).await;
    assert!(fixture
        .store
        .clone()
        .with_usage_capture(Some(config()))
        .await
        .is_err());
    assert!(!fixture
        .db
        .list_collection_names()
        .await
        .unwrap()
        .iter()
        .any(|name| COLLECTIONS.contains(&name.as_str())));
    fixture.prepare().await;
    fixture
        .db
        .run_command(doc! {"createIndexes":CAPTURES,"indexes":[{
        "name":"unsafe_expiry","key":{"expires":1},"expireAfterSeconds":0}]})
        .await
        .unwrap();
    assert!(fixture
        .store
        .clone()
        .with_usage_capture(Some(config()))
        .await
        .is_err());
    assert!(fixture
        .db
        .collection::<Document>(CAPTURES)
        .list_index_names()
        .await
        .unwrap()
        .contains(&"unsafe_expiry".into()));
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn capture_failure_rolls_back_checkpoint_lifecycle_and_parent_fence() {
    let fixture = Fixture::new(true).await;
    let mut parent = fixture.parent("unfunded").await;
    let before = fixture.store.usage(&parent.id).await.unwrap();
    fixture.db.run_command(doc! {"collMod":CAPTURES,"validator":{"reject_new_capture":true},"validationLevel":"strict"}).await.unwrap();
    assert!(fixture
        .store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .is_err());
    parent.revision += 1;
    parent.updated_at = MINUTE.into();
    assert!(fixture.store.save_sandbox(1, parent.clone()).await.is_err());
    assert_eq!(fixture.store.usage(&parent.id).await.unwrap(), before);
    assert_eq!(
        fixture
            .store
            .get_sandbox(&parent.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    assert_eq!(fixture.count(CAPTURES).await, 1);
    assert_eq!(fixture.count(crate::store::sandbox_usage::EVENTS).await, 1);
    let doc = fixture
        .db
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(doc.get_i64("usage_capture_fence_v1").unwrap(), 1);
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn aborted_capture_has_no_source_stream_or_parent_side_effect() {
    let fixture = Fixture::new(true).await;
    let parent = fixture.parent("erased-owner-reference").await;
    let before = fixture.store.usage(&parent.id).await.unwrap();
    let mut session = fixture.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(crate::store::sandbox_usage::record(
        &fixture.store,
        &mut session,
        &parent,
        1,
        MINUTE,
        (1, 1),
        true,
    )
    .await
    .is_ok());
    session.abort_transaction().await.unwrap();
    assert_eq!(fixture.store.usage(&parent.id).await.unwrap(), before);
    assert_eq!(fixture.count(CAPTURES).await, 1);
    assert_eq!(fixture.count(STREAMS).await, 1);
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn concurrent_all_owner_capture_survives_restart_without_funding_queries() {
    let fixture = Fixture::new(true).await;
    for owner in [
        "no-wallet",
        "zero-balance",
        "free",
        "outside-rollout",
        "organization",
        "deleted-owner",
    ] {
        let parent = fixture.parent(owner).await;
        let other = MongoSandboxStore::open(&fixture.client, fixture.db.name())
            .await
            .unwrap()
            .with_usage_capture(Some(config()))
            .await
            .unwrap();
        fixture.commands.lock().unwrap().clear();
        let (left, right) = tokio::join!(
            fixture
                .store
                .checkpoint_usage_one(parent.id.as_str(), MINUTE),
            other.checkpoint_usage_one(parent.id.as_str(), MINUTE)
        );
        left.unwrap();
        right.unwrap();
        for command in fixture.commands.lock().unwrap().iter() {
            for key in ["find", "aggregate", "update", "insert"] {
                assert!(!matches!(
                    command.get_str(key).ok(),
                    Some("sandbox_billing_state" | "auth_subjects" | "users")
                ));
            }
        }
    }
    assert_eq!(fixture.count(STREAMS).await, 6);
    assert_eq!(fixture.count(CAPTURES).await, 12);
    assert_eq!(fixture.count("sandbox_billing_state").await, 0);
    assert_eq!(fixture.count("auth_subjects").await, 0);
    fixture.db.drop().await.unwrap();
}
