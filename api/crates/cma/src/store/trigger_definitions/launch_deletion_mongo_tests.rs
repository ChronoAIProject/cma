use super::*;
use futures_util::TryStreamExt;
use layer2_core::account_deletion::AccountDeletionStore;
use std::collections::BTreeSet;

#[path = "launch_deletion_monitor.rs"]
mod monitor;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fresh_launch_fences_only_the_visitor_and_reads_publisher_authority() {
    for existing_publisher_fence in [false, true] {
        let seed = seeded("publisher_read_only").await;
        let trigger = published(&seed, "publisher_read_only").await;
        let db = seed.client.database(&seed.database);
        crate::store::account_deletions::MongoAccountDeletionStore::open(&db)
            .await
            .unwrap();
        let fences = db.collection::<Document>("sandbox_admissions");
        if existing_publisher_fence {
            fences
                .insert_one(doc! {"_id":trigger.owner.as_str(),"fence":7_i64,
                    "document_schema_version":1_i64})
                .await
                .unwrap();
        }
        let before = fences
            .find_one(doc! {"_id":trigger.owner.as_str()})
            .await
            .unwrap();
        let (store, monitor) = monitor::monitored(&seed).await;
        assert!(store
            .admit_launch(LaunchAdmission {
                launch: launch(&trigger, 'a', 'a'),
                trigger: trigger.clone(),
                events: vec![LaunchEventKind::Created],
            })
            .await
            .unwrap()
            .was_created());
        let commands = monitor.commands();
        let after = fences
            .find_one(doc! {"_id":trigger.owner.as_str()})
            .await
            .unwrap();
        let counts = stats(&seed, &trigger).await;
        db.drop().await.unwrap();
        let mut writes = 0;
        for command in &commands {
            for name in ["insert", "delete", "findAndModify"] {
                assert_ne!(command.get_str(name).ok(), Some("sandbox_admissions"));
            }
            if command.get_str("update").ok() == Some("sandbox_admissions") {
                for update in command.get_array("updates").unwrap() {
                    writes += 1;
                    assert_eq!(
                        update.as_document().unwrap().get_document("q").unwrap(),
                        &doc! {"_id":"visitor"},
                        "Launch must never write the publisher fence: {command:?}"
                    );
                }
            }
        }
        assert_eq!(writes, 1);
        let publisher_hash = layer2_core::account_deletion::subject_hash(&trigger.owner);
        let publisher_read = commands
            .iter()
            .find(|command| {
                command.get_str("find").ok() == Some("account_deletions")
                    && command
                        .get_document("filter")
                        .unwrap()
                        .get_str("subject_hash")
                        .ok()
                        == Some(publisher_hash.as_str())
            })
            .expect("publisher standing is read from the transaction authority");
        assert_eq!(publisher_read.get_bool("autocommit"), Ok(false));
        assert!(publisher_read.contains_key("txnNumber"));
        assert_eq!(
            publisher_read.get_document("projection").unwrap(),
            &doc! {"_id":1}
        );
        assert_eq!(after, before);
        assert_eq!(counts.launches_total, 1);
    }
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn visitor_deletion_committed_before_fence_retries_and_refuses_launch() {
    let seed = seeded("visitor_fence_cutover").await;
    let trigger = published(&seed, "visitor_fence_cutover").await;
    let db = seed.client.database(&seed.database);
    let deletions = crate::store::account_deletions::MongoAccountDeletionStore::open(&db)
        .await
        .unwrap();
    let (store, monitor) = monitor::monitored(&seed).await;
    let visitor = SubjectId::new("visitor");
    monitor.pause_fence(&visitor);
    let admission = LaunchAdmission {
        launch: launch(&trigger, 'a', 'a'),
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    };
    let in_flight = tokio::spawn(async move { store.admit_launch(admission).await });
    monitor.paused().await;
    deletions
        .request(&visitor, "delete-visitor-before-fence", NOW)
        .await
        .unwrap();
    monitor.resume();
    assert_eq!(
        in_flight.await.unwrap(),
        Err(LaunchStoreError::AccountDeletionInProgress)
    );
    let attempts = monitor
        .commands()
        .into_iter()
        .filter(|command| command.get_str("update").ok() == Some("sandbox_admissions"))
        .count();
    assert!(
        attempts >= 2,
        "the stale transaction must retry its visitor fence"
    );
    assert!(owner_launch_ids(&db).await.is_empty());
    assert_eq!(stats(&seed, &trigger).await.launches_total, 0);
    db.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_deletion_does_not_serialize_an_existing_visitor_snapshot() {
    let seed = seeded("publisher_read_snapshot").await;
    let trigger = published(&seed, "publisher_read_snapshot").await;
    let db = seed.client.database(&seed.database);
    let deletions = crate::store::account_deletions::MongoAccountDeletionStore::open(&db)
        .await
        .unwrap();
    let (store, monitor) = monitor::monitored(&seed).await;
    monitor.pause_fence(&SubjectId::new("visitor"));
    let admission = LaunchAdmission {
        launch: launch(&trigger, 'a', 'a'),
        trigger: trigger.clone(),
        events: vec![LaunchEventKind::Created],
    };
    let in_flight = tokio::spawn(async move { store.admit_launch(admission).await });
    monitor.paused().await;
    deletions
        .request(&trigger.owner, "delete-publisher-after-snapshot", NOW)
        .await
        .unwrap();
    assert!(owner_launch_ids(&db).await.is_empty());
    monitor.resume();
    assert!(in_flight.await.unwrap().unwrap().was_created());
    assert_eq!(owner_launch_ids(&db).await.len(), 1);
    assert_eq!(stats(&seed, &trigger).await.launches_total, 1);
    assert_eq!(
        seed.store
            .admit_launch(LaunchAdmission {
                launch: launch(&trigger, 'b', 'b'),
                trigger: trigger.clone(),
                events: vec![LaunchEventKind::Created],
            })
            .await,
        Err(LaunchStoreError::TriggerRetired)
    );
    assert_eq!(owner_launch_ids(&db).await.len(), 1);
    assert_eq!(stats(&seed, &trigger).await.launches_total, 1);
    db.drop().await.unwrap();
}
