//! Missing immutable inputs require independent publisher-erasure evidence.
use super::*;
use layer2_core::account_deletion::{AccountDeletionStore, ErasureStep};
use std::time::{Duration, SystemTime};

async fn draft(f: &Fixture) -> Launch {
    let mut request = f.request("publisher-integrity-draft");
    request.start_if_ready = false;
    f.service
        .create_or_resume_as(&f.identity, &f.public.public_id, request)
        .await
        .unwrap()
        .launch
}

async fn start(f: &Fixture, launch: &Launch) -> Result<Launch, LaunchServiceError> {
    f.service
        .start(
            &f.identity.caller,
            &launch.id,
            StartLaunch {
                trust_confirmed_digest: launch.review.trust_digest().map(str::to_owned),
                acknowledgements: Vec::new(),
            },
        )
        .await
}

async fn remove_inputs(f: &Fixture, revisions: bool) {
    let db = f.seed.client.database(&f.seed.database);
    db.collection::<Document>("trigger_definitions")
        .delete_many(doc! {"_id":f.public.trigger_id.as_str()})
        .await
        .unwrap();
    if revisions {
        db.collection::<Document>("trigger_definition_revisions")
            .delete_many(doc! {"trigger_id":f.public.trigger_id.as_str()})
            .await
            .unwrap();
    }
}

async fn deletion_record(f: &Fixture, step: ErasureStep) {
    let db = f.seed.client.database(&f.seed.database);
    let store = crate::store::account_deletions::MongoAccountDeletionStore::open(&db)
        .await
        .unwrap();
    let at = layer2_core::now_rfc3339();
    store
        .request(&f.seed.owner, "publisher-integrity", &at)
        .await
        .unwrap();
    loop {
        let mut record = store
            .claim_due("publisher-integrity", &at, Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        let expected = record.revision;
        let lease = record.work.lease.as_ref().unwrap().clone();
        record.inventory.triggers = vec![f.public.trigger_id.to_string()];
        let reached = record.step == step;
        let events = if reached {
            record
                .renew(lease.fence, &at, Duration::from_secs(60))
                .unwrap();
            vec![]
        } else {
            vec![record
                .advance(lease.fence, &at, Default::default())
                .unwrap()]
        };
        store
            .commit(record, events, expected, Some(&lease.token))
            .await
            .unwrap();
        if reached {
            return;
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_missing_public_mapping_without_erasure_is_corrupt() {
    let f = Fixture::new(false).await;
    let launch = draft(&f).await;
    remove_inputs(&f, false).await;
    assert!(matches!(
        start(&f, &launch).await,
        Err(LaunchServiceError::Corrupt(_))
    ));
    assert_eq!(
        f.service
            .read(&f.identity.caller, &launch.id)
            .await
            .unwrap(),
        launch
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_missing_mapping_with_durable_revision_fence_is_retired() {
    let f = Fixture::new(false).await;
    let launch = draft(&f).await;
    let db = f.seed.client.database(&f.seed.database);
    db.collection::<Document>("trigger_definition_revisions")
        .update_one(
            doc! {"trigger_id":launch.trigger_id.as_str()},
            doc! {"$set":{
                "account_erasure_fenced":true,
                "account_erasure_fenced_at":mongodb::bson::DateTime::now(),
            }},
        )
        .await
        .unwrap();
    remove_inputs(&f, false).await;
    assert!(matches!(
        start(&f, &launch).await,
        Err(LaunchServiceError::TriggerRetired)
    ));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_malformed_revision_fence_does_not_hide_corruption() {
    for field in ["owner", "account_erasure_fenced_at"] {
        let f = Fixture::new(false).await;
        let launch = draft(&f).await;
        erase_publisher(&f).await;
        let db = f.seed.client.database(&f.seed.database);
        let mut fields = doc! {"account_erasure_fenced":true};
        fields.insert(field, "invalid-erasure-evidence");
        db.collection::<Document>("trigger_definition_revisions")
            .update_one(
                doc! {"trigger_id":launch.trigger_id.as_str()},
                doc! {"$set":fields},
            )
            .await
            .unwrap();
        assert!(
            matches!(
                start(&f, &launch).await,
                Err(LaunchServiceError::Corrupt(_))
            ),
            "{field}"
        );
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_missing_revision_without_erasure_remains_worker_corruption() {
    let f = Arc::new(Fixture::new(false).await);
    let launch = f.launch("missing-frozen-trigger").await;
    remove_inputs(&f, true).await;
    let worker = LaunchWorker::new(
        Arc::new(f.seed.store.clone()),
        Arc::new(Driver(f.clone())),
        LaunchWorkerConfig::new("missing-frozen-trigger-worker"),
    );
    assert!(matches!(
        worker.run_one(&launch.id, SystemTime::now()).await,
        Err(crate::trigger::LaunchWorkerError::Corrupt(_))
    ));
    let retained = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.state, launch.state);
    assert!(retained.failure.is_none());
    f.seed
        .client
        .database(&f.seed.database)
        .drop()
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_missing_inputs_with_durable_erasure_record_are_retired() {
    let f = Arc::new(Fixture::new(false).await);
    let draft = draft(&f).await;
    let launch = f.launch("erased-frozen-trigger").await;
    deletion_record(&f, ErasureStep::EraseObjects).await;
    remove_inputs(&f, true).await;
    assert!(matches!(
        start(&f, &draft).await,
        Err(LaunchServiceError::TriggerRetired)
    ));
    let worker = LaunchWorker::new(
        Arc::new(f.seed.store.clone()),
        Arc::new(Driver(f.clone())),
        LaunchWorkerConfig::new("erased-frozen-trigger-worker"),
    );
    assert!(worker.run_one(&launch.id, SystemTime::now()).await.unwrap());
    let settled = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, LaunchState::FailedTerminal);
    assert_eq!(settled.failure.unwrap().code, "trigger_retired");
    f.seed
        .client
        .database(&f.seed.database)
        .drop()
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_inventory_record_is_not_proof_of_removed_trigger() {
    let f = Arc::new(Fixture::new(false).await);
    let draft = draft(&f).await;
    let launch = f.launch("not-yet-erased-frozen-trigger").await;
    deletion_record(&f, ErasureStep::Inventory).await;
    remove_inputs(&f, true).await;
    assert!(matches!(
        start(&f, &draft).await,
        Err(LaunchServiceError::Corrupt(_))
    ));
    let worker = LaunchWorker::new(
        Arc::new(f.seed.store.clone()),
        Arc::new(Driver(f.clone())),
        LaunchWorkerConfig::new("not-erased-trigger-worker"),
    );
    assert!(matches!(
        worker.run_one(&launch.id, SystemTime::now()).await,
        Err(crate::trigger::LaunchWorkerError::Corrupt(_))
    ));
    f.seed
        .client
        .database(&f.seed.database)
        .drop()
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_corrupt_erasure_record_cannot_hide_missing_inputs() {
    let f = Fixture::new(false).await;
    let launch = draft(&f).await;
    deletion_record(&f, ErasureStep::EraseObjects).await;
    let db = f.seed.client.database(&f.seed.database);
    db.collection::<Document>("account_deletions")
        .update_one(
            doc! {"inventory.triggers":launch.trigger_id.as_str()},
            doc! {"$set":{"subject_hash":"0".repeat(64)}},
        )
        .await
        .unwrap();
    remove_inputs(&f, true).await;
    assert!(matches!(
        start(&f, &launch).await,
        Err(LaunchServiceError::Corrupt(_))
    ));
    f.close().await;
}
