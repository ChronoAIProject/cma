use super::*;
use crate::store::provenance_progress::{initialize, status, COLLECTION};
use layer2_core::sandbox::store::ProvenanceBackfillProgress;

fn worker() -> String {
    uuid::Uuid::new_v4().to_string()
}

async fn counts(store: &MongoSandboxStore) -> Document {
    store
        .client
        .database(&store.agents.namespace().db)
        .collection::<Document>(COLLECTION)
        .find_one(doc! {"_id":"agent-provenance-v1"})
        .await
        .unwrap()
        .unwrap()
}
async fn expire(store: &MongoSandboxStore) {
    store
        .client
        .database(&store.agents.namespace().db)
        .collection::<Document>(COLLECTION)
        .update_one(
            doc! {"_id":"agent-provenance-v1"},
            doc! {"$set":{"lease_until":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn progress_requires_explicit_activation_and_retains_original_facts() {
    let (client, name, store, _, _, request) = legacy_fixture("progress_activation").await;
    let db = client.database(&name);
    let owner = worker();
    let before = store
        .agent_provenance_snapshot(&request.agent_id)
        .await
        .unwrap();
    assert_eq!(
        store
            .durable_provenance_backfill_step(&owner)
            .await
            .unwrap(),
        None
    );
    assert_eq!(status(&db).await.unwrap()["phase"], "inactive");
    initialize(&db).await.unwrap();
    let ready = counts(&store).await;
    initialize(&db).await.unwrap();
    assert_eq!(counts(&store).await, ready);
    assert_eq!(
        store
            .durable_provenance_backfill_step(&owner)
            .await
            .unwrap(),
        Some(ProvenanceBackfillProgress::Advanced)
    );
    let progress = counts(&store).await;
    assert_eq!(
        progress.get_document("counts").unwrap(),
        &doc! {"scanned":1_i64,"recorded":1_i64,"retained":0_i64,"skipped":0_i64,"errors":0_i64}
    );
    let after = store
        .agent_provenance_snapshot(&request.agent_id)
        .await
        .unwrap();
    assert_eq!(before.resource, after.resource);
    assert_eq!(before.first_log, after.first_log);
    assert_eq!(
        after.creation.unwrap(),
        before.creation_provenance(&request.agent_id).unwrap()
    );
    assert_eq!(
        store
            .durable_provenance_backfill_step(&owner)
            .await
            .unwrap(),
        Some(ProvenanceBackfillProgress::SweepFinished)
    );
    let completed = counts(&store).await;
    let receipt_id = completed.get_str("last_receipt").unwrap();
    let receipt = db
        .collection::<Document>(COLLECTION)
        .find_one(doc! {"_id":receipt_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.get_str("phase").unwrap(), "sweep_finished");
    assert_eq!(status(&db).await.unwrap()["global_completion"], false);
    assert_eq!(
        store
            .durable_provenance_backfill_step(&worker())
            .await
            .unwrap(),
        Some(ProvenanceBackfillProgress::Busy)
    );
    initialize(&db).await.unwrap();
    assert_eq!(
        db.collection::<Document>(COLLECTION)
            .find_one(doc! {"_id":receipt_id})
            .await
            .unwrap(),
        Some(receipt)
    );
    assert_eq!(counts(&store).await, completed);
    let public = status(&db).await.unwrap().to_string();
    assert!(!public.contains(request.agent_id.as_str()));
    assert!(!public.contains(&owner));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn progress_concurrency_and_restart_cannot_double_count() {
    let (client, name, store, _, workspace, _) = legacy_fixture("progress_race").await;
    let mut next = admission(&workspace, "race_later");
    next.agent_id = AgentId("zz_race_later".into());
    store.admit_agent(next).await.unwrap();
    let db = client.database(&name);
    initialize(&db).await.unwrap();
    let left_owner = worker();
    let right_owner = worker();
    let (left, right) = tokio::join!(
        store.durable_provenance_backfill_step(&left_owner),
        store.durable_provenance_backfill_step(&right_owner)
    );
    let outcomes = [left.unwrap().unwrap(), right.unwrap().unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|x| **x == ProvenanceBackfillProgress::Advanced)
            .count(),
        1
    );
    let original = counts(&store).await;
    assert_eq!(
        original
            .get_document("counts")
            .unwrap()
            .get_i64("recorded")
            .unwrap(),
        1
    );
    expire(&store).await;
    let takeover = worker();
    assert_eq!(
        store
            .durable_provenance_backfill_step(&takeover)
            .await
            .unwrap(),
        Some(ProvenanceBackfillProgress::Advanced)
    );
    let completed = counts(&store).await;
    assert_eq!(
        completed
            .get_document("counts")
            .unwrap()
            .get_i64("recorded")
            .unwrap(),
        1
    );
    assert_eq!(
        completed
            .get_document("counts")
            .unwrap()
            .get_i64("retained")
            .unwrap(),
        1
    );
    assert!(completed.get_i64("fence").unwrap() > original.get_i64("fence").unwrap());
    for stale in [&left_owner, &right_owner] {
        assert_eq!(
            store.durable_provenance_backfill_step(stale).await.unwrap(),
            Some(ProvenanceBackfillProgress::Busy)
        );
    }
    assert_eq!(counts(&store).await, completed);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn progress_exceptions_remain_visible_and_later_sweeps_reconcile() {
    let (client, name, store, _, workspace, request) = legacy_fixture("progress_corrupt").await;
    let db = client.database(&name);
    let mut next = admission(&workspace, "progress_later");
    next.agent_id = AgentId("zz_later".into());
    store.admit_agent(next.clone()).await.unwrap();
    let original = store
        .agent_log
        .find_one(doc! {"agent_id":request.agent_id.as_str(),"seq":1_i64})
        .await
        .unwrap()
        .unwrap();
    store
        .agent_log
        .update_one(
            doc! {"agent_id":request.agent_id.as_str(),"seq":1_i64},
            doc! {"$set":{"record.appended_at":"2026-09-06T00:00:00Z"}},
        )
        .await
        .unwrap();
    initialize(&db).await.unwrap();
    let owner = worker();
    for _ in 0..2 {
        assert_eq!(
            store
                .durable_provenance_backfill_step(&owner)
                .await
                .unwrap(),
            Some(ProvenanceBackfillProgress::Advanced)
        );
    }
    assert_eq!(
        store
            .durable_provenance_backfill_step(&owner)
            .await
            .unwrap(),
        Some(ProvenanceBackfillProgress::SweepFinished)
    );
    let first = counts(&store).await;
    assert_eq!(
        first.get_document("counts").unwrap(),
        &doc! {"scanned":2_i64,"recorded":0_i64,"retained":1_i64,"skipped":0_i64,"errors":1_i64}
    );
    let collection = db.collection::<Document>(COLLECTION);
    assert_eq!(
        collection
            .count_documents(doc! {"reason":"corrupt"})
            .await
            .unwrap(),
        1
    );
    let receipt_id = first.get_str("last_receipt").unwrap();
    let receipt = collection
        .find_one(doc! {"_id":receipt_id})
        .await
        .unwrap()
        .unwrap();
    // Repair only synthetic source evidence, then trigger the next scheduled sweep.
    store
        .agent_log
        .replace_one(doc! {"_id":original.get("_id").unwrap().clone()}, original)
        .await
        .unwrap();
    collection
        .update_one(
            doc! {"_id":"agent-provenance-v1"},
            doc! {"$set":{"next_sweep_at":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    for _ in 0..2 {
        store
            .durable_provenance_backfill_step(&owner)
            .await
            .unwrap();
    }
    store
        .durable_provenance_backfill_step(&owner)
        .await
        .unwrap();
    let second = counts(&store).await;
    assert_eq!(
        second.get_document("counts").unwrap(),
        &doc! {"scanned":2_i64,"recorded":1_i64,"retained":1_i64,"skipped":0_i64,"errors":0_i64}
    );
    assert_ne!(
        first.get_str("run").unwrap(),
        second.get_str("run").unwrap()
    );
    assert_eq!(
        collection.find_one(doc! {"_id":receipt_id}).await.unwrap(),
        Some(receipt)
    );
    assert_eq!(
        collection
            .count_documents(doc! {"reason":"corrupt"})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn progress_refuses_future_or_missing_control_and_missing_receipts() {
    let (client, name, store, _, _, _) = legacy_fixture("progress_drift").await;
    let db = client.database(&name);
    let owner = worker();
    initialize(&db).await.unwrap();
    let collection = db.collection::<Document>(COLLECTION);
    let original = counts(&store).await;
    collection
        .update_one(
            doc! {"_id":"agent-provenance-v1"},
            doc! {"$set":{"protocol":2_i64}},
        )
        .await
        .unwrap();
    assert!(store
        .durable_provenance_backfill_step(&owner)
        .await
        .is_err());
    assert!(initialize(&db).await.is_err());
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    collection
        .replace_one(doc! {"_id":"agent-provenance-v1"}, original)
        .await
        .unwrap();
    store
        .durable_provenance_backfill_step(&owner)
        .await
        .unwrap();
    store
        .durable_provenance_backfill_step(&owner)
        .await
        .unwrap();
    let control = counts(&store).await;
    collection
        .delete_one(doc! {"_id":control.get_str("last_receipt").unwrap()})
        .await
        .unwrap();
    assert!(store
        .durable_provenance_backfill_step(&owner)
        .await
        .is_err());
    assert!(status(&db).await.is_err());
    collection
        .delete_one(doc! {"_id":"agent-provenance-v1"})
        .await
        .unwrap();
    assert!(store
        .durable_provenance_backfill_step(&owner)
        .await
        .is_err());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn progress_unknown_commit_resumes_without_reclassifying_the_original_effect() {
    let (admin, name, _, _, _, request) = legacy_fixture("progress_unknown").await;
    let db = admin.database(&name);
    initialize(&db).await.unwrap();
    let app = worker();
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":5_i32},"data":{"failCommands":["commitTransaction"],"appName":&app,"writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected unknown progress commit"}}}).await.unwrap();
    let result = store.durable_provenance_backfill_step(&app).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(result.is_err());
    // Simulate a restarted executor after the old holder's expiry.
    expire(&store).await;
    assert_eq!(
        store
            .durable_provenance_backfill_step(&worker())
            .await
            .unwrap(),
        Some(ProvenanceBackfillProgress::SweepFinished)
    );
    let control = counts(&store).await;
    assert_eq!(
        control
            .get_document("counts")
            .unwrap()
            .get_i64("scanned")
            .unwrap(),
        1
    );
    assert_eq!(
        control
            .get_document("counts")
            .unwrap()
            .get_i64("recorded")
            .unwrap(),
        1
    );
    assert_eq!(
        control
            .get_document("counts")
            .unwrap()
            .get_i64("retained")
            .unwrap(),
        0
    );
    assert!(store
        .agent_provenance_snapshot(&request.agent_id)
        .await
        .unwrap()
        .creation
        .is_some());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn progress_interrupted_before_commit_leaves_no_effect_or_counter() {
    let (client, name, store, _, _, _) = legacy_fixture("progress_interrupted").await;
    let db = client.database(&name);
    initialize(&db).await.unwrap();
    let original = counts(&store).await;
    crate::store::provenance_progress::interrupt_for_test(&store, &worker())
        .await
        .unwrap();
    assert_eq!(counts(&store).await, original);
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    store
        .durable_provenance_backfill_step(&worker())
        .await
        .unwrap();
    assert_eq!(
        counts(&store)
            .await
            .get_document("counts")
            .unwrap()
            .get_i64("recorded")
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn progress_driver_failure_does_not_count_or_skip_an_observation() {
    let (admin, name, _, _, _, _) = legacy_fixture("progress_driver").await;
    let db = admin.database(&name);
    initialize(&db).await.unwrap();
    let app = worker();
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let original = counts(&store).await;
    admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1_i32},"data":{"failCommands":["aggregate"],"appName":&app,"errorCode":13_i32}}).await.unwrap();
    let outcome = store.durable_provenance_backfill_step(&app).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(outcome.is_err());
    assert_eq!(counts(&store).await, original);
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    db.drop().await.unwrap();
}
