use super::*;

#[derive(Default)]
struct TransactionTrace {
    starts: usize,
    commits: usize,
    sessions: Vec<mongodb::bson::Document>,
}

type SharedTransactionTrace = Arc<std::sync::Mutex<TransactionTrace>>;

async fn failpoint_fixture() -> (
    Client,
    Client,
    String,
    String,
    Arc<MongoSandboxStore>,
    AgentOpenRequest,
    u64,
    SharedTransactionTrace,
) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app = format!("cma-provider-tools-{suffix}");
    let database = format!("cma_provider_tools_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let trace = Arc::new(std::sync::Mutex::new(TransactionTrace::default()));
    let observed = trace.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            let mut trace = observed.lock().unwrap();
            if matches!(event.command.get_bool("startTransaction"), Ok(true)) {
                trace.starts += 1;
                let session = event.command.get_document("lsid").unwrap().clone();
                if !trace.sessions.contains(&session) {
                    trace.sessions.push(session);
                }
            }
            if event.command_name == "commitTransaction" {
                trace.commits += 1;
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    crate::store::migrations::prepare_bindings_for_test(&client.database(&database)).await;
    let store = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let (_, conversation, request) = tests::setup(&store).await;
    let fence = store
        .get_agent(&conversation.agent_id)
        .await
        .unwrap()
        .unwrap()
        .work
        .next_fence;
    (admin, client, database, app, store, request, fence, trace)
}

async fn disable(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .expect("disable failpoint before assertions");
}

async fn finish_injection(admin: &Client, trace: &SharedTransactionTrace) -> (usize, usize) {
    disable(admin).await;
    let (starts, commits, sessions) = {
        let trace = trace.lock().unwrap();
        (trace.starts, trace.commits, trace.sessions.clone())
    };
    // failCommand can refuse commit before the server executes it. The driver
    // already considers that session committed and cannot send abortTransaction.
    // Kill only the test operation's captured sessions, releasing its locks
    // before fixture reuse or database cleanup; committed receipts are retained.
    assert!(
        !sessions.is_empty(),
        "the injected operation must start a transaction"
    );
    admin
        .database("admin")
        .run_command(doc! {"killSessions": sessions})
        .await
        .expect("release only the injected operation's sessions");
    (starts, commits)
}

async fn wait_for_confirmation(store: &MongoSandboxStore, expected_fence: i64) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let fence = store
                .bindings
                .records
                .find_one(doc! {})
                .await
                .unwrap()
                .unwrap()
                .get_i64("tool_open_fence")
                .unwrap();
            let receipts = store
                .bindings
                .tool_opens
                .count_documents(doc! {})
                .await
                .unwrap();
            assert!(
                fence <= expected_fence,
                "the transaction body must not replay"
            );
            assert!(receipts <= 1, "confirmation must retain one exact receipt");
            if fence == expected_fence && receipts == 1 {
                return;
            }
            // An injected write-concern error skips the successful majority
            // acknowledgement, so an immediate majority read may still be old.
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("committed confirmation must become majority-visible within five seconds");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn mongo_provider_resolve_and_confirm_retry_transient_commit_failure() {
    let (admin, client, database, app, store, request, fence, trace) = failpoint_fixture().await;
    let source = MongoProviderToolSource::new(store.clone());
    for confirm in [false, true] {
        let attachment = if confirm {
            Some(observed_binding(&source, &request, fence).await)
        } else {
            None
        };
        *trace.lock().unwrap() = TransactionTrace::default();
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
                "data": {"failCommands": ["commitTransaction"], "appName": &app,
                    "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"]},
            })
            .await
            .unwrap();
        let result = if let Some(attachment) = &attachment {
            source.confirm_open(attachment, "native-conversation").await
        } else {
            source
                .resolve(&RuntimeHandle("provider-runtime".into()), &request, fence)
                .await
                .map(|binding| assert!(binding.is_some()))
        };
        let (starts, _) = finish_injection(&admin, &trace).await;
        assert_eq!(
            starts, 2,
            "a transient commit restarts the transaction once"
        );
        result.unwrap();
        assert_eq!(
            store
                .bindings
                .tool_opens
                .count_documents(doc! {})
                .await
                .unwrap(),
            u64::from(confirm)
        );
    }
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn mongo_provider_unknown_commit_does_not_restart_or_claim_success() {
    let (admin, client, database, app, store, request, fence, trace) = failpoint_fixture().await;
    let source = MongoProviderToolSource::new(store.clone());
    let attachment = observed_binding(&source, &request, fence).await;
    let before = store
        .bindings
        .records
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap()
        .get_i64("tool_open_fence")
        .unwrap();
    *trace.lock().unwrap() = TransactionTrace::default();
    admin.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": {"times": 5_i32},
        "data": {"failCommands": ["commitTransaction"], "appName": &app,
            "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected provider confirmation ambiguity"}},
    }).await.unwrap();
    let result = source
        .confirm_open(&attachment, "native-conversation")
        .await;
    let (starts, commits) = finish_injection(&admin, &trace).await;
    assert!(matches!(result, Err(AdapterError::Unavailable(_))));
    assert_eq!(
        starts, 1,
        "an unknown commit must not restart the transaction body"
    );
    assert_eq!(commits, 5, "only the same transaction's commit is retried");
    wait_for_confirmation(&store, before + 1).await;
    assert_eq!(
        store
            .bindings
            .records
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap()
            .get_i64("tool_open_fence")
            .unwrap(),
        before + 1,
        "unknown commit may apply once but must not replay the transaction"
    );
    // The server committed despite the ambiguous response. Only the explicit
    // later invocation may observe and validate that exact receipt.
    assert_eq!(
        store
            .bindings
            .tool_opens
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    source
        .confirm_open(&attachment, "native-conversation")
        .await
        .unwrap();
    assert_eq!(
        store
            .bindings
            .tool_opens
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert!(matches!(
        source
            .confirm_open(&attachment, "another-conversation")
            .await,
        Err(AdapterError::Rejected(_))
    ));
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn mongo_provider_nontransient_error_does_not_retry_or_publish_receipt() {
    for confirm in [false, true] {
        for (command, code) in [("update", 121_i32), ("commitTransaction", 2_i32)] {
            let (admin, client, database, app, store, request, fence, trace) =
                failpoint_fixture().await;
            let source = MongoProviderToolSource::new(store.clone());
            let attachment = if confirm {
                Some(observed_binding(&source, &request, fence).await)
            } else {
                None
            };
            let before = store
                .bindings
                .records
                .find_one(doc! {})
                .await
                .unwrap()
                .unwrap();
            *trace.lock().unwrap() = TransactionTrace::default();
            admin
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
                    "data": {"failCommands": [command], "appName": &app,
                        "errorCode": code, "errorLabels": []},
                })
                .await
                .unwrap();
            let result = if let Some(attachment) = &attachment {
                source.confirm_open(attachment, "native-conversation").await
            } else {
                source
                    .resolve(&RuntimeHandle("provider-runtime".into()), &request, fence)
                    .await
                    .map(|_| ())
            };
            let (starts, commits) = finish_injection(&admin, &trace).await;
            assert_eq!(
                starts, 1,
                "nontransient {command} must not restart the body"
            );
            assert_eq!(commits, usize::from(command == "commitTransaction"));
            assert_eq!(result, Err(unavailable(())), "failed {command}");
            assert_eq!(
                store
                    .bindings
                    .tool_opens
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                0
            );
            let after = store
                .bindings
                .records
                .find_one(doc! {})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                before.get("tool_open_fence"),
                after.get("tool_open_fence"),
                "failed {command} must not publish the transaction's fence"
            );
            client.database(&database).drop().await.unwrap();
        }
    }
}
