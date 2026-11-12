use super::*;
use futures_util::TryStreamExt;
use layer2_core::account_deletion::AccountDeletionStore;
use std::collections::BTreeSet;

async fn owner_launch_ids(db: &mongodb::Database) -> BTreeSet<String> {
    db.collection::<Document>(LAUNCHES)
        .find(doc! {"record.owner":"visitor"})
        .projection(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get_str("_id").unwrap().to_owned())
        .collect()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn deletion_and_launch_admission_serialize_on_the_owner_fence() {
    let seed = seeded("deletion_launch_race").await;
    let trigger = published(&seed, "deletion_launch_race").await;
    let db = seed.client.database(&seed.database);
    let deletions = crate::store::account_deletions::MongoAccountDeletionStore::open(&db)
        .await
        .unwrap();
    let candidates = ('a'..='f')
        .map(|key| LaunchAdmission {
            launch: launch(&trigger, key, key),
            trigger: trigger.clone(),
            events: vec![LaunchEventKind::Created],
        })
        .collect::<Vec<_>>();
    let owner = SubjectId::new("visitor");
    let (cutover, admissions) = tokio::join!(
        async {
            deletions
                .request(&owner, "delete-visitor", NOW)
                .await
                .unwrap();
            owner_launch_ids(&db).await
        },
        futures_util::future::join_all(
            candidates
                .into_iter()
                .map(|admission| seed.store.admit_launch(admission))
        ),
    );
    let admitted = admissions
        .into_iter()
        .filter_map(|result| match result {
            Ok(result) => Some(result),
            Err(LaunchStoreError::AccountDeletionInProgress) => None,
            Err(error) => panic!("unexpected Launch admission error: {error}"),
        })
        .count();
    assert_eq!(
        db.collection::<Document>(LAUNCHES)
            .count_documents(doc! {"record.owner":"visitor"})
            .await
            .unwrap(),
        admitted as u64
    );
    assert_eq!(owner_launch_ids(&db).await, cutover);
    let fresh = LaunchAdmission {
        launch: launch(&trigger, '0', '0'),
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    };
    assert_eq!(
        seed.store.admit_launch(fresh).await,
        Err(LaunchStoreError::AccountDeletionInProgress)
    );
    assert_eq!(owner_launch_ids(&db).await, cutover);
    let mut other = launch(&trigger, '1', '1');
    other.owner = SubjectId::new("other-visitor");
    assert!(seed
        .store
        .admit_launch(LaunchAdmission {
            launch: other,
            trigger,
            events: vec![LaunchEventKind::Created]
        })
        .await
        .is_ok());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_deletion_refuses_fresh_launch_without_changing_admission_counts() {
    let seed = seeded("publisher_deletion_launch").await;
    let trigger = published(&seed, "publisher_deletion_launch").await;
    let db = seed.client.database(&seed.database);
    let deletions = crate::store::account_deletions::MongoAccountDeletionStore::open(&db)
        .await
        .unwrap();
    let accepted = LaunchAdmission {
        launch: launch(&trigger, 'a', 'a'),
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    };
    let receipt = seed.store.admit_launch(accepted.clone()).await.unwrap();
    let before = seed
        .store
        .trigger(&trigger.trigger_id, &trigger.owner)
        .await
        .unwrap()
        .unwrap();
    deletions
        .request(&trigger.owner, "delete-publisher", NOW)
        .await
        .unwrap();
    assert!(matches!(
        seed.store.admit_launch(accepted).await.unwrap(),
        LaunchAdmissionResult::Existing(_)
    ));
    let fresh = LaunchAdmission {
        launch: launch(&trigger, 'b', 'b'),
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    };
    assert!(matches!(
        seed.store.admit_launch(fresh).await,
        Err(LaunchStoreError::TriggerRetired)
    ));
    assert_eq!(
        seed.store
            .trigger(&trigger.trigger_id, &trigger.owner)
            .await
            .unwrap()
            .unwrap()
            .stats,
        before.stats
    );
    assert_eq!(owner_launch_ids(&db).await.len(), 1);
    assert!(matches!(receipt, LaunchAdmissionResult::Created(_)));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_deletion_and_launch_admission_serialize_without_post_cutover_counts() {
    let seed = seeded("publisher_deletion_launch_race").await;
    let trigger = published(&seed, "publisher_deletion_launch_race").await;
    let db = seed.client.database(&seed.database);
    let deletions = crate::store::account_deletions::MongoAccountDeletionStore::open(&db)
        .await
        .unwrap();
    let candidates = ('a'..='f')
        .map(|key| LaunchAdmission {
            launch: launch(&trigger, key, key),
            trigger: trigger.clone(),
            events: vec![LaunchEventKind::Created],
        })
        .collect::<Vec<_>>();
    let (cutover, admissions) = tokio::join!(
        async {
            deletions
                .request(&trigger.owner, "delete-publisher-race", NOW)
                .await
                .unwrap();
            let count = owner_launch_ids(&db).await;
            let stats = seed
                .store
                .trigger(&trigger.trigger_id, &trigger.owner)
                .await
                .unwrap()
                .unwrap()
                .stats;
            (count, stats)
        },
        futures_util::future::join_all(
            candidates
                .into_iter()
                .map(|admission| seed.store.admit_launch(admission))
        )
    );
    let admitted = admissions
        .into_iter()
        .filter_map(|result| match result {
            Ok(result) => Some(result),
            Err(LaunchStoreError::TriggerRetired) => None,
            Err(error) => panic!("unexpected publisher admission error: {error}"),
        })
        .count();
    assert_eq!(owner_launch_ids(&db).await, cutover.0);
    assert_eq!(cutover.0.len(), admitted);
    assert_eq!(
        seed.store
            .trigger(&trigger.trigger_id, &trigger.owner)
            .await
            .unwrap()
            .unwrap()
            .stats,
        cutover.1
    );
    db.drop().await.unwrap();
}
