use super::{abort_handoff::*, *};
use mongodb::options::ClientOptions;

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn removing_barrier_is_a_negative_control_for_delayed_delete_after_completion() {
    let abort_client = client().await;
    let (database, ledger, predecessor) = prepared(abort_client.clone()).await;
    let (point, ready, resume) = checkpoint();
    let db = database.clone();
    let running = tokio::spawn(async move {
        abort::execute_inner(
            &catalog::activity(),
            abort_client,
            db,
            &identity(),
            &image(),
            predecessor.fence,
            abort::Hooks {
                draining: Some(point),
                ..Default::default()
            },
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), ready)
        .await
        .unwrap()
        .unwrap();
    // Restore the old protocol's absence of a barrier in this disposable fixture.
    database
        .collection::<Document>(ledger::COLLECTION)
        .delete_one(doc! {"_id":"destructive_ddl"})
        .await
        .unwrap();
    expire(&database).await;
    assert_eq!(
        rebuild(client().await, &database).await.unwrap()["created_indexes"],
        0
    );
    assert!(ledger.completed().await.unwrap());
    resume.send(()).unwrap();
    assert!(running.await.unwrap().is_err());
    assert!(ledger.completed().await.unwrap());
    assert_eq!(index_names(&database).await, vec!["_id_", "retained"]);
    assert_eq!(
        rebuild(client().await, &database).await.unwrap_err(),
        "completed Query index manifest drifted"
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB with test commands"]
async fn unknown_abort_claim_and_settlement_commits_reconcile_atomically() {
    let admin = client().await;
    let app = format!("abort-commit-{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = Some(app.clone());
    let actor = Client::with_options(options).unwrap();
    let (database, ledger, predecessor) = prepared(actor).await;
    let injection = doc! {"configureFailPoint":"failCommand","mode":{"times":5},
    "data":{"failCommands":["commitTransaction"],"appName":app,
    "writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired","errmsg":"fixture abort commit ambiguity"}}};
    admin
        .database("admin")
        .run_command(injection.clone())
        .await
        .unwrap();
    let claimed = ledger.claim_abort(&identity(), predecessor.fence).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let lease = claimed.unwrap();
    let pending = ledger.status().await.unwrap();
    assert_eq!(pending["destructive_ddl"]["state"], "pending");
    assert_eq!(pending["destructive_ddl"]["fence"], lease.fence);
    assert_eq!(pending["recent_attempts"].as_array().unwrap().len(), 2);
    expire(&database).await;
    let different_step = ledger::Ledger::new(admin.clone(), admin.database(database.name()));
    different_step.initialize().await.unwrap();
    assert!(different_step
        .claim(&identity())
        .await
        .err()
        .unwrap()
        .starts_with("destructive index operation is unsettled"));
    admin
        .database("admin")
        .run_command(injection)
        .await
        .unwrap();
    // No DDL was dispatched; settlement is permitted even after lease expiry.
    let settled = ledger
        .settle_abort(&lease, false, "fixture cancelled before dispatch")
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    settled.unwrap();
    let status = ledger.status().await.unwrap();
    assert_eq!(status["destructive_ddl"]["state"], "settled");
    assert_eq!(status["recent_attempts"][0]["outcome"], "failed");
    assert_eq!(status["completed"], false);
    assert_eq!(
        rebuild(admin, &database).await.unwrap()["created_indexes"],
        0
    );
    assert!(ledger.completed().await.unwrap());
    assert!(ledger
        .settle_abort(&lease, false, "stale settlement")
        .await
        .is_err());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn barrier_drift_refuses_takeover_and_settlement_without_hiding_history() {
    let (database, ledger, predecessor) = prepared(client().await).await;
    let lease = ledger
        .claim_abort(&identity(), predecessor.fence)
        .await
        .unwrap();
    let rows = database.collection::<Document>(ledger::COLLECTION);
    let original = rows
        .find_one(doc! {"_id":"destructive_ddl"})
        .await
        .unwrap()
        .unwrap();
    expire(&database).await;
    for (field, value) in [
        ("checksum", "wrong".into()),
        ("state", "unknown".into()),
        ("state", mongodb::bson::Bson::Array(vec!["settled".into()])),
        ("attempt", "missing".into()),
        ("fence", 0_i64.into()),
        ("extra", true.into()),
    ] {
        let mut changed = original.clone();
        changed.insert(field, value);
        rows.replace_one(doc! {"_id":"destructive_ddl"}, changed)
            .await
            .unwrap();
        assert!(ledger.preflight().await.is_err(), "{field}");
        assert!(ledger.claim(&identity()).await.is_err(), "{field}");
        assert!(
            ledger
                .settle_abort(&lease, false, "must not settle drift")
                .await
                .is_err(),
            "{field}"
        );
        assert!(!ledger.completed().await.unwrap());
    }
    rows.replace_one(doc! {"_id":"destructive_ddl"}, original)
        .await
        .unwrap();
    ledger
        .settle_abort(&lease, false, "fixture stopped before dispatch")
        .await
        .unwrap();
    assert_eq!(
        ledger.status().await.unwrap()["recent_attempts"][0]["outcome"],
        "failed"
    );
    assert!(index_names(&database)
        .await
        .iter()
        .any(|name| name == catalog::activity().index_name()));
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn refusal_before_dispatch_settles_barrier_and_preserves_unrelated_indexes() {
    let actor = client().await;
    let (database, ledger, predecessor) = prepared(actor.clone()).await;
    database
        .run_command(abort::drop_command(&catalog::activity()).unwrap())
        .await
        .unwrap();
    assert!(abort::execute(
        &catalog::activity(),
        actor.clone(),
        database.clone(),
        &identity(),
        &image(),
        predecessor.fence
    )
    .await
    .is_err());
    assert_eq!(
        ledger.status().await.unwrap()["destructive_ddl"]["state"],
        "settled"
    );
    assert_eq!(index_names(&database).await, vec!["_id_", "retained"]);
    assert_eq!(
        rebuild(actor, &database).await.unwrap()["created_indexes"],
        1
    );
    database.drop().await.unwrap();
}
