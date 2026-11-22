use super::*;
use cma_service_auth::mongodb_authority::MongoAuthorityMode;
use mongodb::bson::{doc, Document};

mod abort_barrier;
mod abort_handoff;
mod candidates;

#[test]
fn app_user_preflight_proves_build_and_abort_without_exposing_grants() {
    let mode = MongoAuthorityMode::SharedSchema;
    let query = catalog::activity();
    let status = |grants| doc! {"authInfo":{"authenticatedUsers":[{"user":"private-user-canary","db":"fixture"}],"authenticatedUserPrivileges":grants}};
    let actions = vec![
        "find",
        "insert",
        "update",
        "remove",
        "createCollection",
        "createIndex",
        "listIndexes",
        "collMod",
        "collStats",
        "dropIndex",
        "listCollections",
    ];
    let grant = |actions| doc! {"resource":{"db":"fixture","collection":""},"actions":actions};
    let full = status(vec![grant(actions.clone())]);
    let good = authority::preflight_report(&full, "fixture", mode, std::slice::from_ref(&query));
    assert_eq!(good["preflight_passed"], true);
    assert_eq!(good["checks"][0]["create_index"], true);
    assert_eq!(good["checks"][0]["drop_index"], true);
    assert!(!good.to_string().contains("private-user-canary"));
    assert!(authority::verify_effective(&full, "fixture", mode, &["activity"], true).is_ok());
    // Explicit shared-schema admission does not weaken the isolated runtime contract.
    assert!(authority::verify_effective(
        &full,
        "fixture",
        MongoAuthorityMode::Isolated,
        &["activity"],
        true
    )
    .is_err());
    for missing in ["createIndex", "dropIndex", "collStats", "insert"] {
        let narrowed = actions
            .iter()
            .copied()
            .filter(|a| *a != missing)
            .collect::<Vec<_>>();
        let denied = status(vec![grant(narrowed)]);
        let output =
            authority::preflight_report(&denied, "fixture", mode, std::slice::from_ref(&query));
        assert_eq!(output["preflight_passed"], false);
        assert_eq!(
            output["blocked_collections"],
            serde_json::json!(["activity"])
        );
        assert!(
            authority::verify_effective(&denied, "fixture", mode, &["activity"], true).is_err()
        );
    }
    let list_only = status(vec![grant(vec!["listIndexes"])]);
    let output = authority::preflight_report(&list_only, "fixture", mode, &[query]);
    assert_eq!(output["checks"][0]["create_index"], false);
    assert_eq!(output["checks"][0]["drop_index"], false);
    let mut other_collection = grant(actions.clone());
    other_collection
        .get_document_mut("resource")
        .unwrap()
        .insert("collection", "agents");
    let wrong_target = authority::preflight_report(
        &status(vec![other_collection]),
        "fixture",
        mode,
        &[catalog::activity()],
    );
    assert_eq!(wrong_target["checks"][0]["create_index"], false);
    assert_eq!(
        wrong_target["blocked_collections"],
        serde_json::json!(["activity"])
    );
    for resource in [
        doc! {"db":"foreign","collection":""},
        doc! {"anyResource":true},
    ] {
        let mut invalid = grant(actions.clone());
        invalid.insert("resource", resource);
        assert!(authority::verify_effective(
            &status(vec![invalid]),
            "fixture",
            mode,
            &["activity"],
            true
        )
        .is_err());
    }
}

#[test]
fn verification_checks_every_branch_and_never_accepts_incomplete_topology() {
    use super::super::index_plan::{sanitize, verifies};
    let target = "activity_recorded_at_id_desc";
    let good = doc! {"stage":"IXSCAN","indexName":target};
    for bad in [
        doc! {"stage":"COLLSCAN"},
        doc! {"stage":"SORT","inputStage":good.clone()},
        doc! {"stage":"FUTURE_SCAN","indexName":target},
        doc! {"stage":"IXSCAN","indexName":"wrong"},
        doc! {"stage":"FETCH"},
    ] {
        let plan = doc! {"stage":"SHARD_MERGE","shards":[{"shardName":"canary","winningPlan":good.clone()},{"connectionString":"canary","winningPlan":bad}]};
        let sanitized = sanitize(&plan, target, &["recorded_at", "_id"]);
        assert!(!verifies(&sanitized, target));
        assert!(!sanitized.to_string().contains("canary"));
    }
    assert!(verifies(
        &sanitize(
            &doc! {"queryPlan":{"stage":"FETCH","inputStage":good}},
            target,
            &[]
        ),
        target
    ));
    assert!(!verifies(
        &sanitize(&doc! {"stage":"SHARD_MERGE","shards":[]}, target, &[]),
        target
    ));
}

fn identity() -> ledger::Identity {
    ledger::Identity::new(&"a".repeat(40), &image()).unwrap()
}
fn image() -> String {
    format!(
        "us-central1-docker.pkg.dev/fixture/release/cma-api@sha256:{}",
        "b".repeat(64)
    )
}
async fn fixture() -> (Client, Database) {
    let client = Client::with_uri_str(
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI"),
    )
    .await
    .unwrap();
    let database = client.database(&format!(
        "cma_index_toolkit_{}",
        uuid::Uuid::new_v4().simple()
    ));
    database.create_collection("activity").await.unwrap();
    (client, database)
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn named_query_inspection_build_verify_and_completed_abort_refusal() {
    let (client, database) = fixture().await;
    let query = catalog::activity();
    database
        .collection::<Document>("activity")
        .insert_many((0..1000).map(|i| doc! {"recorded_at":i,"record":{"detail":"private-canary"}}))
        .await
        .unwrap();
    let before = inspection::inspect_query(&database, &query).await.unwrap();
    assert_eq!(before["metadata"]["document_count"], 1000);
    assert_eq!(before["index_present"], false);
    assert_eq!(before["first_page_explain"]["uses_expected_index"], false);
    let built = build::execute(&query, client.clone(), database.clone(), &identity(), 60)
        .await
        .unwrap();
    assert_eq!(built["created_indexes"], 1);
    let after = inspection::inspect_query(&database, &query).await.unwrap();
    println!(
        "query={} fixture_rows={} docs_before={} docs_after={} keys_after={}",
        query.name,
        after["metadata"]["document_count"],
        before["first_page_explain"]["documents_examined"],
        after["first_page_explain"]["documents_examined"],
        after["first_page_explain"]["keys_examined"]
    );
    assert_eq!(after["first_page_explain"]["uses_expected_index"], true);
    assert!(!after.to_string().contains("private-canary"));
    let repeated = build::execute(&query, client.clone(), database.clone(), &identity(), 60)
        .await
        .unwrap();
    assert_eq!(repeated["outcome"], "already_completed");
    assert!(
        abort::execute(&query, client, database.clone(), &identity(), &image(), 1)
            .await
            .is_err()
    );
    assert_eq!(
        database
            .collection::<Document>("activity")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1000
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn exact_index_after_lost_completion_reconciles_through_new_engine() {
    let (client, database) = fixture().await;
    let query = catalog::activity();
    let ledger =
        ledger::Ledger::for_step(client.clone(), database.clone(), query.migration().unwrap());
    ledger.initialize().await.unwrap();
    let lease = ledger.claim(&identity()).await.unwrap();
    database
        .run_command(doc! {"createIndexes":"activity","indexes":[query.index.clone()]})
        .await
        .unwrap();
    ledger
        .finish(&lease, Err("simulated lost client acknowledgement"))
        .await
        .unwrap();
    let result = build::execute(&query, client, database.clone(), &identity(), 60)
        .await
        .unwrap();
    assert_eq!(result["created_indexes"], 0);
    assert!(ledger.completed().await.unwrap());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn abort_fences_predecessor_drops_only_target_and_preserves_failed_history() {
    let (client, database) = fixture().await;
    let query = catalog::activity();
    let ledger =
        ledger::Ledger::for_step(client.clone(), database.clone(), query.migration().unwrap());
    ledger.initialize().await.unwrap();
    let lease = ledger.claim(&identity()).await.unwrap();
    database.run_command(doc!{"createIndexes":"activity","indexes":[query.index.clone(),{"name":"retained","key":{"other":1}}]}).await.unwrap();
    assert!(abort::execute(
        &query,
        client.clone(),
        database.clone(),
        &identity(),
        &image(),
        lease.fence
    )
    .await
    .is_err());
    ledger
        .finish(&lease, Err("simulated lost build acknowledgement"))
        .await
        .unwrap();
    assert!(abort::execute(
        &query,
        client.clone(),
        database.clone(),
        &identity(),
        &image(),
        lease.fence + 1
    )
    .await
    .is_err());
    let dropped = abort::execute(
        &query,
        client.clone(),
        database.clone(),
        &identity(),
        &image(),
        lease.fence,
    )
    .await
    .unwrap();
    assert_eq!(dropped["outcome"], "drop_acknowledged");
    assert!(!ledger.completed().await.unwrap());
    assert!(ledger.finish(&lease, Ok(1)).await.is_err());
    let indexes = database
        .run_command(doc! {"listIndexes":"activity"})
        .await
        .unwrap();
    assert!(indexes.to_string().contains("retained"));
    assert!(!indexes.to_string().contains(query.index_name()));
    assert_eq!(
        build::execute(&query, client, database.clone(), &identity(), 60)
            .await
            .unwrap()["created_indexes"],
        1
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn collated_collection_and_index_drift_refuse_before_index_ddl() {
    let (client, database) = fixture().await;
    database
        .collection::<Document>("activity")
        .drop()
        .await
        .unwrap();
    database
        .run_command(doc! {"create":"activity","collation":{"locale":"en"}})
        .await
        .unwrap();
    let query = catalog::activity();
    assert!(
        build::execute(&query, client.clone(), database.clone(), &identity(), 60)
            .await
            .is_err()
    );
    let indexes = database
        .run_command(doc! {"listIndexes":"activity"})
        .await
        .unwrap();
    assert!(!indexes.to_string().contains(query.index_name()));
    database
        .collection::<Document>("activity")
        .drop()
        .await
        .unwrap();
    database.create_collection("activity").await.unwrap();
    let mut drift = query.index.clone();
    drift.insert("sparse", true);
    database
        .run_command(doc! {"createIndexes":"activity","indexes":[drift]})
        .await
        .unwrap();
    assert!(
        build::execute(&query, client, database.clone(), &identity(), 60)
            .await
            .is_err()
    );
    assert!(inspection::inspect_query(&database, &query).await.is_err());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB with test commands"]
async fn long_command_renews_lease_and_refuses_a_competing_builder() {
    let (client, database) = fixture().await;
    let query = catalog::activity();
    client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},
        "data":{"failCommands":["createIndexes"],"blockConnection":true,"blockTimeMS":35000}})
        .await
        .unwrap();
    let task_client = client.clone();
    let task_database = database.clone();
    let task_query = query.clone();
    let running = tokio::spawn(async move {
        build::execute(&task_query, task_client, task_database, &identity(), 60).await
    });
    tokio::time::sleep(Duration::from_secs(31)).await;
    assert!(
        build::execute(&query, client, database.clone(), &identity(), 60)
            .await
            .is_err()
    );
    assert_eq!(running.await.unwrap().unwrap()["created_indexes"], 1);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB with test commands"]
async fn lost_lease_cannot_complete_and_reconciled_operation_can_resume() {
    let (client, database) = fixture().await;
    let query = catalog::activity();
    client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},
        "data":{"failCommands":["createIndexes"],"blockConnection":true,"blockTimeMS":7000}})
        .await
        .unwrap();
    let task_client = client.clone();
    let task_database = database.clone();
    let task_query = query.clone();
    let running = tokio::spawn(async move {
        build::execute(&task_query, task_client, task_database, &identity(), 60).await
    });
    tokio::time::sleep(Duration::from_secs(2)).await;
    database
        .collection::<Document>(ledger::COLLECTION)
        .update_one(
            doc! {"_id":"runner"},
            doc! {"$set":{"expires_at":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert!(running.await.unwrap().is_err());
    let ledger =
        ledger::Ledger::for_step(client.clone(), database.clone(), query.migration().unwrap());
    assert!(!ledger.completed().await.unwrap());
    // The test's bounded failpoint has ended before a new attempt is allowed.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        build::execute(&query, client, database.clone(), &identity(), 60)
            .await
            .is_ok()
    );
    assert!(ledger.completed().await.unwrap());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn owner_page_reuses_engine_and_never_outputs_sampled_owner() {
    let (client, database) = fixture().await;
    let query = catalog::agents_owner();
    database.create_collection("agents").await.unwrap();
    let empty = inspection::inspect_query(&database, &query).await.unwrap();
    assert_eq!(empty["first_page_explain"]["available"], false);
    database.collection::<Document>("agents").insert_many((0..1000).map(|i|doc!{
        "_id":format!("agent_{i:05}"),"owner":format!("private-owner-canary-{}",i%20),"record":{"detail":"private-canary"}})).await.unwrap();
    let before = inspection::inspect_query(&database, &query).await.unwrap();
    assert_eq!(before["index_present"], false);
    assert_eq!(
        build::execute(&query, client.clone(), database.clone(), &identity(), 60)
            .await
            .unwrap()["created_indexes"],
        1
    );
    let after = inspection::inspect_query(&database, &query).await.unwrap();
    println!(
        "query={} fixture_rows={} docs_before={} docs_after={} keys_after={}",
        query.name,
        after["metadata"]["document_count"],
        before["first_page_explain"]["documents_examined"],
        after["first_page_explain"]["documents_examined"],
        after["first_page_explain"]["keys_examined"]
    );
    assert_eq!(after["first_page_explain"]["uses_expected_index"], true);
    assert_eq!(
        before["first_page_explain"]["filter_sha256"],
        after["first_page_explain"]["filter_sha256"]
    );
    assert!(!after.to_string().contains("private-owner-canary"));
    assert!(
        after["first_page_explain"]["documents_examined"]
            .as_i64()
            .unwrap()
            <= 51
    );
    let activity = catalog::activity();
    assert_eq!(
        build::execute(&activity, client, database.clone(), &identity(), 60)
            .await
            .unwrap()["created_indexes"],
        1
    );
    database.drop().await.unwrap();
}
