use super::*;
use crate::store::agent_log_segments as segments;

#[test]
fn segment_step_is_explicit_and_preserves_the_original_cli_and_receipt_fields() {
    let values = |args: &[&str]| {
        args.iter()
            .map(|value| (*value).into())
            .collect::<Vec<String>>()
    };
    assert_eq!(
        invocation(&values(&["--migration-plan"])).unwrap().0,
        Step::Activity
    );
    let args = values(&["--migration-plan", agent_log_segments::ID]);
    assert_eq!(invocation(&args).unwrap().0, Step::AgentLogSegments);
    assert!(invocation(&values(&["--migration-plan", "unknown"])).is_err());
    assert_eq!(Step::AgentLogSegments.collections(), segments::COLLECTIONS);
    assert_eq!(Step::Activity.effect_field(), "created_indexes");
    assert_eq!(Step::LedgerCollection.effect_field(), "created_indexes");
    assert_eq!(
        Step::AgentLogSegments.effect_field(),
        "applied_schema_steps"
    );
    assert_ne!(Step::AgentLogSegments.checksum(), Step::Activity.checksum());
    assert_eq!(Step::AgentLogSegments.checksum().len(), 64);
}

#[test]
fn segment_migration_authority_allows_additive_schema_only() {
    let grants = authority::role_for("fixture", true, Step::AgentLogSegments)
        .get_array("privileges")
        .unwrap()
        .clone();
    let status = |grants| {
        doc! {"authInfo":{"authenticatedUsers":[{"user":"fixture","db":"fixture"}],
        "authenticatedUserPrivileges":grants}}
    };
    for mode in [
        MongoAuthorityMode::Isolated,
        MongoAuthorityMode::SharedSchema,
    ] {
        assert!(authority::verify_for(
            &status(grants.clone()),
            "fixture",
            mode,
            true,
            Step::AgentLogSegments
        )
        .is_ok());
    }
    let read = authority::role_for("fixture", false, Step::AgentLogSegments)
        .get_array("privileges")
        .unwrap()
        .clone();
    assert!(authority::verify_for(
        &status(read.clone()),
        "fixture",
        MongoAuthorityMode::Isolated,
        false,
        Step::AgentLogSegments
    )
    .is_ok());
    assert!(authority::verify_for(
        &status(read),
        "fixture",
        MongoAuthorityMode::Isolated,
        true,
        Step::AgentLogSegments
    )
    .is_err());
    for action in ["remove", "dropCollection", "dropIndex", "collMod"] {
        let mut changed = grants.clone();
        changed[1]
            .as_document_mut()
            .unwrap()
            .get_array_mut("actions")
            .unwrap()
            .push(action.into());
        assert!(authority::verify_for(
            &status(changed),
            "fixture",
            MongoAuthorityMode::Isolated,
            true,
            Step::AgentLogSegments
        )
        .is_err());
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn schema_plan_is_read_only_and_partial_ddl_resumes_under_the_shared_lease() {
    let (client, database, legacy) = fixture().await;
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::AgentLogSegments);
    let plan = agent_log_segments::inspect(&database).await.unwrap();
    assert_eq!(plan["writes"], false);
    assert_eq!(plan["activates_agents"], false);
    assert!(plan["pending_steps"].as_u64().unwrap() > 0);
    assert!(database.list_collection_names().await.unwrap().is_empty());
    ledger.initialize().await.unwrap();
    legacy.initialize().await.unwrap();
    let abandoned = ledger.claim(&identity()).await.unwrap();
    assert!(legacy.claim(&identity()).await.is_err());
    assert!(agent_log_segments::apply_next(&database).await.unwrap());
    expire(&database).await;
    let result = agent_log_segments::apply(&ledger, &database, &identity())
        .await
        .unwrap();
    assert_eq!(result["outcome"], "completed");
    assert!(result["applied_schema_steps"].as_i64().unwrap() > 0);
    assert!(result.get("created_indexes").is_none());
    assert_eq!(segments::schema::pending_count(&database).await.unwrap(), 0);
    assert!(ledger.finish(&abandoned, Ok(0)).await.is_err());
    assert!(ledger.completed().await.unwrap());
    assert!(!legacy.completed().await.unwrap());
    assert_eq!(
        agent_log_segments::apply(&ledger, &database, &identity())
            .await
            .unwrap()["outcome"],
        "already_completed"
    );
    let status = ledger.status().await.unwrap();
    assert_eq!(status["recent_attempts"].as_array().unwrap().len(), 2);
    assert!(status["recent_attempts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row.get("created_indexes").is_none()));
    assert_eq!(
        database
            .collection::<Document>("agents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        database
            .collection::<Document>("agent_log")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn completed_schema_drift_and_unrecognized_history_refuse_before_effects() {
    let (client, database, _) = fixture().await;
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::AgentLogSegments);
    agent_log_segments::apply(&ledger, &database, &identity())
        .await
        .unwrap();
    database
        .collection::<Document>(segments::SEGMENTS)
        .drop_index("agent_log_segment_ordinal")
        .await
        .unwrap();
    assert!(agent_log_segments::apply(&ledger, &database, &identity())
        .await
        .is_err());
    assert_eq!(segments::schema::pending_count(&database).await.unwrap(), 1);
    database
        .collection::<Document>(ledger::COLLECTION)
        .insert_one(doc! {
            "_id":"manifest:future", "schema":1_i64,"kind":"manifest","migration":"future",
            "checksum":"unrecognized", "completed":false
        })
        .await
        .unwrap();
    assert!(ledger.preflight().await.is_err());
    assert!(agent_log_segments::apply(&ledger, &database, &identity())
        .await
        .is_err());
    assert_eq!(segments::schema::pending_count(&database).await.unwrap(), 1);
    database.drop().await.unwrap();
}
