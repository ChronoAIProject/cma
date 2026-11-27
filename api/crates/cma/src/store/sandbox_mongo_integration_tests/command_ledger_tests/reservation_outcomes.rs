use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn reservation_requires_acknowledged_abort_and_recovers_unknown_commit_by_key() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let name = format!("cma_command_outcomes_{}", uuid::Uuid::new_v4().simple());
    let admin = Client::with_uri_str(&uri).await.unwrap().database("admin");
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(name.clone());
    let client = Client::with_options(options).unwrap();
    let (store, agent, parent) = fixture(&name, &client).await;
    let original = request(&agent, &parent, "same-key");
    let scope = original.scope();
    let key = &original.key_sha256;
    let mut stale = original.clone();
    stale.expected_agent_revision += 1;

    assert_eq!(
        store.reserve_command(stale.clone()).await,
        Err(CommandLedgerError::KnownAborted)
    );
    assert_eq!(store.get_admitted_command(&scope, key).await.unwrap(), None);

    admin
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
            "data": { "failCommands": ["abortTransaction"], "appName": &name, "errorCode": 2_i32 },
        })
        .await
        .unwrap();
    let lost_abort = store.reserve_command(stale).await;
    admin
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    assert_eq!(lost_abort, Err(CommandLedgerError::Unavailable));
    assert_eq!(store.get_admitted_command(&scope, key).await.unwrap(), None);

    admin
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": &name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "fixture commit acknowledgement unavailable",
                },
            },
        })
        .await
        .unwrap();
    let lost_commit = store.reserve_command(original.clone()).await;
    admin
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    assert_eq!(lost_commit, Err(CommandLedgerError::Unavailable));
    let receipt = store
        .get_admitted_command(&scope, key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.receipt.envelope.command_id,
        original.envelope.command_id
    );
    assert_eq!(receipt.receipt.response_id, original.response_id);
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );

    client.database(&name).drop().await.unwrap();
}
