use super::*;
use crate::store::sandbox_mongo::{is_transient, start_transaction, TRANSACTION_ATTEMPTS};
use layer2_core::sandbox::store::{AgentStore, SandboxStore};
use layer2_core::sandbox::{SandboxHome, SandboxStatus};
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;
use mongodb::Client;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Clone, Copy)]
enum Completion {
    Release,
    Revoke,
    ReplaceBoot,
    Exhaust,
}

async fn observed_binding(
    source: &MongoProviderToolSource,
    request: &AgentOpenRequest,
    fence: u64,
) -> HostedProviderToolBinding {
    let mut binding = source
        .resolve(&RuntimeHandle("provider-runtime".into()), request, fence)
        .await
        .unwrap()
        .unwrap();
    binding
        .observe(layer3_adapter::ToolProxyStatus {
            request: binding.scope().3.clone(),
            boot_id: "b".repeat(32),
        })
        .unwrap();
    binding
}

async fn contended(confirm: bool, completion: Completion) {
    let (client, database, store) = super::super::mongo_tests::fixture().await;
    let (binding, conversation, request) = tests::setup(&store).await;
    if matches!(completion, Completion::ReplaceBoot) {
        let mut parent = store
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let revision = parent.revision;
        let mut home = SandboxHome::reserve(
            &parent.id,
            HomeVolumePolicy {
                mode: Default::default(),
                storage_class: "storage".into(),
                capacity_gib: 64,
                template_name: "seed".into(),
                template_generation: "seed-original".into(),
            },
        )
        .unwrap();
        home.bind(HomeVolumeReceipt {
            native_authority: None,
            volume_id: home.request.volume_id.clone(),
            request_sha256: home.request.computed_sha256(),
            storage_reference: "original-home".into(),
            storage_generation: "original-volume".into(),
        })
        .unwrap();
        parent.lifecycle.home = Some(Box::new(home));
        parent.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
        parent.revision += 1;
        store.save_sandbox(revision, parent).await.unwrap();
    }
    let fence = store
        .get_agent(&conversation.agent_id)
        .await
        .unwrap()
        .unwrap()
        .work
        .next_fence;
    let conflicts = Arc::new(AtomicUsize::new(0));
    let observed = conflicts.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Failed(event) = event {
            if event.command_name == "update" && is_transient(&event.failure) {
                observed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }));
    let contender = Client::with_options(options).unwrap();
    let source = MongoProviderToolSource::new(Arc::new(
        MongoSandboxStore::open(&contender, &database)
            .await
            .unwrap(),
    ));
    let attachment = if confirm {
        Some(observed_binding(&source, &request, fence).await)
    } else {
        None
    };
    let before = store
        .bindings
        .records
        .find_one(doc! {"_id": &binding.binding_id})
        .await
        .unwrap()
        .unwrap();
    let mut holder = client.start_session().await.unwrap();
    start_transaction(&mut holder).await.unwrap();
    let update = if matches!(completion, Completion::Revoke | Completion::ReplaceBoot) {
        let mut parent = store
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let previous = parent.clone();
        match completion {
            Completion::Revoke => parent
                .begin_retirement(super::super::mongo_tests::NOW.into())
                .unwrap(),
            Completion::ReplaceBoot => {
                let runtime = parent.runtime.as_mut().unwrap();
                runtime.runtime_id = "replacement-provider-runtime".into();
                runtime.provision_generation = Some(2);
                runtime.boot_id = Some("c".repeat(32));
                parent.lifecycle.runtime_generation_high_water = 2;
                parent.revision += 1;
            }
            _ => unreachable!(),
        }
        previous
            .validate_runtime_generation_successor(&parent)
            .unwrap();
        doc! {"$set": crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()}
    } else {
        // The exact same document/fence written by durable reopen output.
        doc! {"$inc": {"agent_conversation_reopen_fence": 1_i64}}
    };
    store
        .sandboxes
        .update_one(doc! {"_id": binding.sandbox_id.as_str()}, update)
        .session(&mut holder)
        .await
        .unwrap();

    if let Some(attachment) = &attachment {
        // Reproduce the old single-attempt boundary with a genuine write
        // conflict, before exercising the public retrying operation.
        let mut single = contender.start_session().await.unwrap();
        start_transaction(&mut single).await.unwrap();
        let result = source
            .confirm_in(attachment, "native-conversation", &mut single)
            .await;
        let _ = single.abort_transaction().await;
        assert!(matches!(result, Err(transaction::Error::Driver(error)) if is_transient(&error)));
        assert_eq!(
            store
                .bindings
                .tool_opens
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    conflicts.store(0, Ordering::SeqCst);
    let task_source = source.clone();
    let task = tokio::spawn(async move {
        if let Some(attachment) = attachment {
            task_source
                .confirm_open(&attachment, "native-conversation")
                .await
                .map(|()| None)
        } else {
            task_source
                .resolve(&RuntimeHandle("provider-runtime".into()), &request, fence)
                .await
        }
    });
    if !matches!(completion, Completion::Exhaust) {
        let reached = tokio::time::timeout(Duration::from_secs(3), async {
            while conflicts.load(Ordering::SeqCst) == 0 && !task.is_finished() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        holder.commit_transaction().await.unwrap();
        reached.unwrap();
    }
    let result = tokio::time::timeout(Duration::from_secs(3), task).await;
    if matches!(completion, Completion::Exhaust) {
        holder.abort_transaction().await.unwrap();
    }
    let result = result.unwrap().unwrap();
    let count = conflicts.load(Ordering::SeqCst);
    assert!(
        count > 0,
        "test must observe a real MongoDB transaction conflict"
    );
    match completion {
        Completion::Release => {
            let attachment = result.unwrap();
            assert_eq!(attachment.is_some(), !confirm);
            assert_eq!(
                store
                    .bindings
                    .tool_opens
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                u64::from(confirm)
            );
            if confirm {
                let row = store
                    .bindings
                    .tool_opens
                    .find_one(doc! {})
                    .await
                    .unwrap()
                    .unwrap();
                let receipt: OpenRecord = read_record(&row).unwrap();
                assert_eq!(receipt.conversation, "native-conversation");
                assert_eq!(receipt.boot_id, "b".repeat(32));
            }
        }
        Completion::Revoke | Completion::ReplaceBoot => {
            assert!(matches!(result, Err(AdapterError::Rejected(_))));
            assert_eq!(
                store
                    .bindings
                    .tool_opens
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                0
            );
            let parent = store
                .get_sandbox(&binding.sandbox_id)
                .await
                .unwrap()
                .unwrap();
            if matches!(completion, Completion::Revoke) {
                assert_eq!(parent.status, SandboxStatus::Retiring);
            } else {
                assert_eq!(parent.status, SandboxStatus::Active);
                assert_eq!(parent.runtime.unwrap().boot_id, Some("c".repeat(32)));
            }
        }
        Completion::Exhaust => {
            assert!(matches!(result, Err(AdapterError::Unavailable(_))));
            assert_eq!(count, TRANSACTION_ATTEMPTS);
            assert_eq!(
                store
                    .bindings
                    .tool_opens
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                0
            );
        }
    }
    if !matches!(completion, Completion::Release) {
        let after = store
            .bindings
            .records
            .find_one(doc! {"_id": &binding.binding_id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            before.get("tool_open_fence"),
            after.get("tool_open_fence"),
            "aborted attempts must not publish a binding fence or receipt"
        );
    }
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_provider_resolve_retries_real_lineage_write_conflict() {
    contended(false, Completion::Release).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_provider_confirm_retries_real_reopen_output_write_conflict() {
    contended(true, Completion::Release).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_provider_confirm_retry_rechecks_revoked_authority() {
    contended(true, Completion::Revoke).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_provider_confirm_retry_refuses_replaced_runtime_boot() {
    contended(true, Completion::ReplaceBoot).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_provider_confirm_stops_after_bounded_conflicts_without_receipt() {
    contended(true, Completion::Exhaust).await;
}

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
