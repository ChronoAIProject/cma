use super::*;

async fn failpoint_fixture() -> (Client, Client, String, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_queued_input_{suffix}");
    let app = format!("cma-queued-input-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    (admin, Client::with_options(options).unwrap(), name, app)
}

async fn disable(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn queued_input_commits_while_old_dispatch_transaction_is_paused() {
    let (admin, client, name, app) = failpoint_fixture().await;
    let (store, agent, parent, mut request, _) = queued(&name, &admin).await;
    let worker = MongoSandboxStore::open(&client, &name).await.unwrap();
    let claimed = store
        .claim_next_command(claim_request(
            &agent,
            "old-dispatch",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let claim = claimed.work.lease.as_ref().unwrap().claim.clone();
    request.cancellation.target.expected_command_revision = claimed.revision;
    let mutation = message_mutation(&agent, &parent, &claim);
    let configured = admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":1_i32},
        "data":{"failCommands":["update"], "appName":&app, "blockConnection":true, "blockTimeMS":5000_i32},
    }).await.unwrap();
    let count = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    let dispatch = tokio::spawn(async move { worker.commit_agent_conversation(mutation).await });
    let reached = admin
        .database("admin")
        .run_command(
            doc! {"waitForFailPoint":"failCommand", "timesEntered":count+1, "maxTimeMS":5000_i32},
        )
        .await;
    let cancelled = store.edit_queued_input(request).await;
    disable(&admin).await;
    let dispatch = dispatch.await.unwrap();
    reached.unwrap();
    assert!(dispatch.is_err());
    assert_eq!(
        applied(cancelled.unwrap()).response.status,
        AgentResponseStatus::Queued
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        store
            .agent_commands
            .count_documents(doc! {"record.work.dispatch":{"$ne":mongodb::bson::Bson::Null}})
            .await
            .unwrap(),
        0
    );
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn queued_input_unknown_commit_recovers_one_receipt_and_one_effect() {
    let (admin, client, name, app) = failpoint_fixture().await;
    let (store, _, _, request, source) = queued(&name, &client).await;
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"], "appName":&app,
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected queued edit ambiguity"}},
    }).await.unwrap();
    let result = store.edit_queued_input(request.clone()).await;
    disable(&admin).await;
    assert_eq!(result.unwrap_err(), CommandLedgerError::Unavailable);
    crate::store::test_visibility::majority_barrier(&admin, &name).await;
    let command = &request.cancellation.command;
    let committed = store
        .get_admitted_command(&command.scope(), &command.key_sha256)
        .await
        .unwrap()
        .expect("the commit was durable before any replay");
    let original = applied(store.edit_queued_input(request.clone()).await.unwrap());
    assert_eq!(original.record, committed);
    assert_eq!(original.response.revision, source.revision + 1);
    assert_eq!(
        original.response.queued_input.as_ref().unwrap().text,
        request.text
    );
    let second = MongoSandboxStore::open(&admin, &name).await.unwrap();
    assert_eq!(
        applied(second.edit_queued_input(request).await.unwrap()),
        original
    );
    assert_eq!(original.response.status, AgentResponseStatus::Queued);
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        2
    );
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    admin.database(&name).drop().await.unwrap();
}
