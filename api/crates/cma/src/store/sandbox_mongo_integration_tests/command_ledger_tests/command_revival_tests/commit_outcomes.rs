use super::*;

async fn failpoint_client() -> (Client, Client, String, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_revival_{suffix}");
    let app_name = format!("cma-revival-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    (
        admin,
        Client::with_options(options).unwrap(),
        name,
        app_name,
    )
}

async fn disable(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn revival_expiry_during_transaction_io_rolls_back_command_marker_parent_and_event() {
    let (admin, client, name, app_name) = failpoint_client().await;
    let fixture = Fixture::new(&client, &name).await;
    let before = fixture.documents().await;
    let proof = fixture.proof(Duration::from_millis(500)).await;
    let mutation = fixture.mutation(proof);
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":1_i32},
        "data":{"failCommands":["insert"],"appName":&app_name,"blockConnection":true,"blockTimeMS":1000_i32},
    }).await.unwrap();
    let result = fixture.store.commit(mutation).await;
    disable(&admin).await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(
        fixture
            .store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(fixture.documents().await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn concurrent_command_refusal_prevents_revival_from_a_transaction_snapshot() {
    let (admin, client, name, app_name) = failpoint_client().await;
    let fixture = Fixture::new(&client, &name).await;
    let replica = MongoSandboxStore::open(&admin, &name).await.unwrap();
    let proof = fixture.proof(MAX_MEMBERSHIP_AGE).await;
    let mutation = fixture.mutation(proof);
    let configured = admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":1_i32},
        "data":{"failCommands":["update"],"appName":&app_name,"blockConnection":true,"blockTimeMS":2000_i32},
    }).await.unwrap();
    let count = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    let store = fixture.store.clone();
    let pending = tokio::spawn(async move { store.commit(mutation).await });
    let reached = admin
        .database("admin")
        .run_command(doc! {
            "waitForFailPoint":"failCommand", "timesEntered":count+1, "maxTimeMS":5000_i32,
        })
        .await;
    let refused = replica
        .reject_unclaimed_command(
            &fixture.command.receipt.envelope.command_id,
            fixture.command.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await;
    let result = pending.await.unwrap();
    disable(&admin).await;
    reached.unwrap();
    refused.unwrap();
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(
        fixture
            .store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        fixture.store.get_sandbox(&fixture.parent.id).await.unwrap(),
        Some(fixture.parent.clone())
    );
    let row = fixture
        .store
        .agent_commands
        .find_one(doc! {"_id":fixture.command.receipt.envelope.command_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!row.contains_key("last_command_revival"));
    assert_eq!(
        fixture
            .store
            .get_command(&fixture.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentResponseStatus::Failed
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn ambiguous_revival_commit_recovers_after_authority_expiry_without_repeating_event() {
    let (admin, client, name, app_name) = failpoint_client().await;
    let fixture = Fixture::new(&client, &name).await;
    let proof = fixture.proof(Duration::from_millis(500)).await;
    let mutation = fixture.mutation(proof.clone());
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand","mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":&app_name,
            "blockConnection":true,"blockTimeMS":200_i32,
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous revival commit"}},
    }).await.unwrap();
    let result = fixture.store.commit(mutation).await;
    disable(&admin).await;
    result.unwrap();
    assert!(proof.require_current().is_err());
    let audits = fixture
        .store
        .command_authorization_audits(
            &fixture.command.receipt.envelope.command_id,
            &PageRequest::new(20, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audits.items.len(), 1);
    assert_eq!(audits.items[0].resources.len(), 2);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .command_authorization_audits(
                &fixture.command.receipt.envelope.command_id,
                &PageRequest::new(20, None).unwrap()
            )
            .await
            .unwrap(),
        audits
    );
    let before = fixture.documents().await;
    let proof = fixture.proof(MAX_MEMBERSHIP_AGE).await;
    let SandboxLifecycleCommandResult::Accepted(outcome) =
        fixture.store.schedule_command_revival(proof).await.unwrap()
    else {
        panic!("accepted revival should remain scheduled")
    };
    assert_eq!(
        outcome.disposition,
        SandboxLifecycleCommandDisposition::AlreadyScheduled
    );
    assert_eq!(fixture.documents().await, before);
    client.database(&name).drop().await.unwrap();
}
