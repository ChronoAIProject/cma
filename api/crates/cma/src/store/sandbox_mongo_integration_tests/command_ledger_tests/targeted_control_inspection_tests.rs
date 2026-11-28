use super::*;
use layer2_core::sandbox::agent_io::TargetedControlInspection;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn targeted_inspection_proves_stale_absence_and_recovers_existing_receipts_across_replicas() {
    let (client, name) = database("targeted_inspection").await;
    let (one, agent, mut request, source) = composed(&name, &client).await;
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        two.inspect_targeted_control(request.clone()).await.unwrap(),
        TargetedControlInspection::Missing
    );
    let mut future = request.clone();
    future.command.expected_agent_revision += 1;
    assert_eq!(
        two.inspect_targeted_control(future).await.unwrap(),
        TargetedControlInspection::Missing
    );
    let admitted = one.reserve_targeted_control(request.clone()).await.unwrap();
    let current = finish_and_start(&one, &agent, &source).await;
    request.observed_agent_revision = current.revision;
    assert_eq!(
        two.inspect_targeted_control(request.clone()).await.unwrap(),
        TargetedControlInspection::Existing(Box::new(admitted))
    );
    request.command.key_sha256 = command_key_digest("absent-original").unwrap();
    request.command.envelope.command_id = AgentCommandId::generate();
    let before = snapshot(&one).await;
    assert_eq!(
        two.inspect_targeted_control(request.clone()).await.unwrap(),
        TargetedControlInspection::NotAdmitted
    );
    let after = snapshot(&one).await;
    assert_eq!(
        &before[2..],
        &after[2..],
        "no command, order or audit was created"
    );
    assert_eq!(one.get_agent(&agent.id).await.unwrap(), Some(current));
    assert_eq!(
        one.lookup_targeted_control(request.clone()).await.unwrap(),
        None
    );
    assert!(one.reserve_targeted_control(request.clone()).await.is_err());
    request.target.turn_sequence += 1;
    assert_eq!(
        two.inspect_targeted_control(request).await.unwrap_err(),
        CommandLedgerError::NotAdmissible
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn targeted_inspection_never_proves_refusal_for_stale_authority_or_recreated_agent() {
    let (client, name) = database("targeted_inspection_authority").await;
    let (store, agent, mut request, _) = composed(&name, &client).await;
    let mut current = agent.clone();
    current.revision += 1;
    store
        .save_agent(agent.revision, current.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .inspect_targeted_control(request.clone())
            .await
            .unwrap_err(),
        CommandLedgerError::Conflict
    );
    request.observed_agent_revision = current.revision;
    let mut expired = request.clone();
    expired.command.authority_deadline = Some(tokio::time::Instant::now());
    assert_eq!(
        store.inspect_targeted_control(expired).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    let mut missing = request.clone();
    missing.target.response_id = AgentResponseId::generate();
    assert_eq!(
        store.inspect_targeted_control(missing).await.unwrap_err(),
        CommandLedgerError::NotFound
    );
    current.created_at =
        layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(1));
    replace_agent_fixture(&store, &current).await;
    assert_eq!(
        store
            .inspect_targeted_control(request.clone())
            .await
            .unwrap_err(),
        CommandLedgerError::NotFound
    );
    request.agent_created_at = current.created_at;
    assert_eq!(
        store.inspect_targeted_control(request).await.unwrap_err(),
        CommandLedgerError::NotFound
    );
    client.database(&name).drop().await.unwrap();
}

async fn failpoint_fixture() -> (Client, Client, String, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_targeted_inspection_{suffix}");
    let app = format!("cma-targeted-inspection-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
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
async fn targeted_committed_refusal_conflicts_with_an_older_inflight_admission() {
    let (admin, client, name, app) = failpoint_fixture().await;
    let (store, mut agent, mut request, _) = composed(&name, &admin).await;
    let old_store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let stale_request = request.clone();
    // First update is the source fence, after reading the absent key, current
    // resources and bound source in the old admission transaction's snapshot.
    let configured = admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
            "data": { "failCommands": ["update"], "appName": &app,
                "blockConnection": true, "blockTimeMS": 5000_i32 },
        })
        .await
        .unwrap();
    let count = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    let admission = tokio::spawn({
        let request = stale_request.clone();
        async move { old_store.reserve_targeted_control(request).await }
    });
    let reached = admin
        .database("admin")
        .run_command(doc! {
            "waitForFailPoint": "failCommand", "timesEntered": count + 1, "maxTimeMS": 5000_i32,
        })
        .await;
    let previous = agent.revision;
    agent.revision += 1;
    let saved = store.save_agent(previous, agent.clone()).await;
    request.observed_agent_revision = agent.revision;
    let inspection = store.inspect_targeted_control(request.clone()).await;
    disable(&admin).await;
    let admission = admission.await.unwrap();
    reached.unwrap();
    saved.unwrap();
    assert_eq!(inspection.unwrap(), TargetedControlInspection::NotAdmitted);
    assert_eq!(admission.unwrap_err(), CommandLedgerError::ControlBusy);
    // The delayed attempt exhausted its retry budget. A fresh attempt using
    // the same request must read the newer revision and refuse admission.
    assert_eq!(
        store
            .reserve_targeted_control(stale_request)
            .await
            .unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(store.lookup_targeted_control(request).await.unwrap(), None);
    assert_eq!(
        store
            .agent_commands
            .count_documents(doc! { "explicit_control": { "$exists": true } })
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn targeted_refusal_unknown_or_failed_commit_remains_uncertain() {
    let (admin, client, name, app) = failpoint_fixture().await;
    let (store, mut agent, mut request, _) = composed(&name, &client).await;
    let previous = agent.revision;
    agent.revision += 1;
    store.save_agent(previous, agent.clone()).await.unwrap();
    request.observed_agent_revision = agent.revision;
    for unknown in [true, false] {
        let mut data = doc! { "failCommands": ["commitTransaction"], "appName": &app };
        if unknown {
            data.insert("writeConcernError", doc! { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected refusal commit ambiguity" });
        } else {
            data.insert("errorCode", 2_i32);
        }
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": { "times": 5_i32 }, "data": data,
            })
            .await
            .unwrap();
        let result = store.inspect_targeted_control(request.clone()).await;
        disable(&admin).await;
        assert_eq!(result.unwrap_err(), CommandLedgerError::Unavailable);
        assert_eq!(
            store
                .inspect_targeted_control(request.clone())
                .await
                .unwrap(),
            TargetedControlInspection::NotAdmitted
        );
    }
    assert_eq!(
        store
            .agent_commands
            .count_documents(doc! { "explicit_control": { "$exists": true } })
            .await
            .unwrap(),
        0
    );
    admin.database(&name).drop().await.unwrap();
}
