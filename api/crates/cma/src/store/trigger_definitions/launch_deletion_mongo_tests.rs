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
