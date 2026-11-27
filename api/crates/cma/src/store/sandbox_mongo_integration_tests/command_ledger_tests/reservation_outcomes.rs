use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn reservation_requires_acknowledged_abort_and_recovers_unknown_commit_by_key() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let name = format!("cma_command_outcomes_{}", uuid::Uuid::new_v4().simple());
    let admin = Client::with_uri_str(&uri).await.unwrap().database("admin");
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(name.clone());
    let attempts = Arc::new(AtomicU64::new(0));
    let observed_attempts = attempts.clone();
    let commit_replies = Arc::new(AtomicU64::new(0));
    let observed_commits = commit_replies.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| match event {
        CommandEvent::Succeeded(event)
            if event.command_name == "commitTransaction"
                && event.reply.contains_key("writeConcernError") =>
        {
            observed_commits.fetch_add(1, Ordering::Relaxed);
        }
        CommandEvent::Started(event)
            if event.command_name == "find"
                && event
                    .command
                    .get_str("find")
                    .is_ok_and(|name| name == "agent_commands")
                && matches!(event.command.get_bool("autocommit"), Ok(false))
                && event.command.get_document("filter").is_ok_and(|filter| {
                    filter.contains_key("scope") && filter.contains_key("key_sha256")
                }) =>
        {
            observed_attempts.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }));
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
            "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
            "data": {
                "failCommands": ["insert"],
                "appName": &name,
                "errorCode": 11000_i32,
                "errorLabels": ["UnknownTransactionCommitResult"],
            },
        })
        .await
        .unwrap();
    let before = attempts.load(Ordering::Relaxed);
    let mixed_error = store.reserve_command(original.clone()).await;
    admin
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    assert_eq!(mixed_error, Err(CommandLedgerError::Unavailable));
    assert_eq!(store.get_admitted_command(&scope, key).await.unwrap(), None);
    assert_eq!(attempts.load(Ordering::Relaxed), before + 1);

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
    let before = attempts.load(Ordering::Relaxed);
    let commits_before = commit_replies.load(Ordering::Relaxed);
    let committed = request(&agent, &parent, "commit-key");
    let lost_commit = store.reserve_command(committed.clone()).await;
    admin
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    assert_eq!(lost_commit, Err(CommandLedgerError::Unavailable));
    assert_eq!(attempts.load(Ordering::Relaxed), before + 1);
    assert!(commit_replies.load(Ordering::Relaxed) > commits_before);
    crate::store::test_visibility::majority_barrier(admin.client(), &name).await;
    let receipt = store
        .get_admitted_command(&committed.scope(), &committed.key_sha256)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.receipt.envelope.command_id,
        committed.envelope.command_id
    );
    assert_eq!(receipt.receipt.response_id, committed.response_id);
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );

    client.database(&name).drop().await.unwrap();
}
