async fn failpoint_client() -> (Client, Client, String, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_sandbox_controls_{suffix}");
    let app = format!("cma-controls-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    (admin, Client::with_options(options).unwrap(), name, app)
}
async fn disable(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn sandbox_controls_expiry_during_event_insert_rolls_back_all_effects() {
    let (admin, client, name, app) = failpoint_client().await;
    let f = Fixture::new(&client, &name).await;
    let record = f.admit("expire").await.unwrap();
    let proof = f.proof(&record, Duration::from_millis(500)).await;
    let before = f.documents().await;
    admin.database("admin").run_command(doc! { "configureFailPoint": "failCommand", "mode": { "times": 1_i32 }, "data": { "failCommands": ["insert"], "appName": &app, "blockConnection": true, "blockTimeMS": 1000_i32 } }).await.unwrap();
    let result = f.store.apply_sandbox_control(&record, proof).await;
    disable(&admin).await;
    assert_eq!(result, Err(SandboxControlError::Conflict));
    assert_eq!(f.documents().await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn sandbox_controls_policy_revocation_during_transaction_cannot_commit() {
    let (admin, client, name, app) = failpoint_client().await;
    let f = Fixture::new(&client, &name).await;
    let record = f.admit("policy").await.unwrap();
    let proof = f.proof(&record, MAX_MEMBERSHIP_AGE).await;
    let replica = MongoSandboxStore::open(&admin, &name).await.unwrap();
    let configured = admin.database("admin").run_command(doc! { "configureFailPoint": "failCommand", "mode": { "times": 1_i32 }, "data": { "failCommands": ["update"], "appName": &app, "blockConnection": true, "blockTimeMS": 2000_i32 } }).await.unwrap();
    let count = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    let store = f.store.clone();
    let work = record.clone();
    let pending = tokio::spawn(async move { store.apply_sandbox_control(&work, proof).await });
    let reached = admin.database("admin").run_command(doc! { "waitForFailPoint": "failCommand", "timesEntered": count + 1, "maxTimeMS": 5000_i32 }).await;
    let snapshot = replica
        .resource_access_snapshot(&record.envelope.target())
        .await
        .unwrap();
    let share = snapshot.shares[0].clone();
    let access =
        ResourceAccessService::new(Arc::new(replica), Arc::new(Membership(MAX_MEMBERSHIP_AGE)));
    let revoked = access
        .change_share(
            &context("owner"),
            Some("fixture"),
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-held".into(),
            },
        )
        .await;
    let result = pending.await.unwrap();
    disable(&admin).await;
    reached.unwrap();
    revoked.unwrap();
    assert_eq!(result, Err(SandboxControlError::Conflict));
    assert_eq!(
        f.store.get_sandbox(&f.parent.id).await.unwrap(),
        Some(f.parent)
    );
    assert_eq!(
        f.store
            .sandbox_control(&record.envelope.command_id)
            .await
            .unwrap(),
        Some(record)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn sandbox_controls_ambiguous_commit_recovers_receipt_without_repeated_transition() {
    let (admin, client, name, app) = failpoint_client().await;
    let f = Fixture::new(&client, &name).await;
    let record = f.admit("ambiguous").await.unwrap();
    let proof = f.proof(&record, MAX_MEMBERSHIP_AGE).await;
    admin.database("admin").run_command(doc! { "configureFailPoint": "failCommand", "mode": { "times": 5_i32 }, "data": { "failCommands": ["commitTransaction"], "appName": &app, "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous control commit" } } }).await.unwrap();
    let result = f.store.apply_sandbox_control(&record, proof).await;
    disable(&admin).await;
    assert_eq!(result, Err(SandboxControlError::Unavailable));
    crate::store::test_visibility::majority_barrier(&admin, &name).await;
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let stored = reopened
        .sandbox_control(&record.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        stored.status,
        SandboxControlStatus::Applied { .. }
    ));
    let before = f.documents().await;
    assert_eq!(f.admit("ambiguous").await.unwrap(), stored);
    // The additive admission guard may advance; retained aggregate, command,
    // event and Agent history remain identical across retry.
    let after = f.documents().await;
    assert_eq!(before.1, after.1);
    assert_eq!(before.2, after.2);
    assert_eq!(before.3, after.3);
    let mut old = before.0;
    let mut new = after.0;
    old.remove("resource_share_guard");
    new.remove("resource_share_guard");
    assert_eq!(old, new);
    client.database(&name).drop().await.unwrap();
}
