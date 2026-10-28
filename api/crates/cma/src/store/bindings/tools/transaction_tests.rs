use super::*;
use crate::store::sandbox_mongo::{is_transient, start_transaction, TRANSACTION_ATTEMPTS};
use layer2_core::sandbox::store::{AgentStore, SandboxStore};
use layer2_core::sandbox::SandboxStatus;
use mongodb::event::command::CommandEvent;
use mongodb::options::ClientOptions;
use mongodb::{Client, EventHandler};
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
        match completion {
            Completion::Revoke => parent.status = SandboxStatus::Paused,
            Completion::ReplaceBoot => {
                parent.runtime.as_mut().unwrap().boot_id = Some("c".repeat(32));
            }
            _ => unreachable!(),
        }
        parent.revision += 1;
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
                assert_eq!(parent.status, SandboxStatus::Paused);
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

async fn failpoint_fixture() -> (
    Client,
    Client,
    String,
    String,
    Arc<MongoSandboxStore>,
    AgentOpenRequest,
    u64,
) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app = format!("cma-provider-tools-{suffix}");
    let database = format!("cma_provider_tools_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
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
    (admin, client, database, app, store, request, fence)
}

async fn disable(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .expect("disable failpoint before assertions");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn mongo_provider_resolve_and_confirm_retry_transient_commit_failure() {
    let (admin, client, database, app, store, request, fence) = failpoint_fixture().await;
    let source = MongoProviderToolSource::new(store.clone());
    for confirm in [false, true] {
        let attachment = if confirm {
            Some(observed_binding(&source, &request, fence).await)
        } else {
            None
        };
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
        disable(&admin).await;
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
    let (admin, client, database, app, store, request, fence) = failpoint_fixture().await;
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
    admin.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": {"times": 5_i32},
        "data": {"failCommands": ["commitTransaction"], "appName": &app,
            "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected provider confirmation ambiguity"}},
    }).await.unwrap();
    let result = source
        .confirm_open(&attachment, "native-conversation")
        .await;
    disable(&admin).await;
    assert!(matches!(result, Err(AdapterError::Unavailable(_))));
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
    let (admin, client, database, app, store, request, fence) = failpoint_fixture().await;
    let source = MongoProviderToolSource::new(store.clone());
    for confirm in [false, true] {
        let attachment = if confirm {
            Some(observed_binding(&source, &request, fence).await)
        } else {
            None
        };
        for (command, code) in [("update", 121_i32), ("commitTransaction", 2_i32)] {
            let before = store
                .bindings
                .records
                .find_one(doc! {})
                .await
                .unwrap()
                .unwrap();
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
            disable(&admin).await;
            // Retrying this one-shot failure would incorrectly become success.
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
        }
    }
    client.database(&database).drop().await.unwrap();
}
