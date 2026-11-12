use super::*;
use crate::store::sandbox_mongo::TRANSACTION_ATTEMPTS;
use std::sync::atomic::AtomicBool;

#[derive(Default)]
struct Trace {
    commands: Mutex<Vec<(String, Document)>>,
    snapshot_times: Mutex<Vec<mongodb::bson::Timestamp>>,
    pause_shares: AtomicBool,
    entered: tokio::sync::Notify,
    resume: (Mutex<bool>, std::sync::Condvar),
}

async fn observed(trace: &Arc<Trace>) -> Client {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = Some(format!("access-snapshot-{}", uuid::Uuid::new_v4()));
    let trace = trace.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| match event {
        CommandEvent::Started(event) => {
            let pause = matches!(event.command.get_str("find"), Ok("resource_shares"))
                && trace.pause_shares.swap(false, Ordering::SeqCst);
            trace
                .commands
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
            if pause {
                trace.entered.notify_one();
                let (ready, wake) = &trace.resume;
                let (_ready, timeout) = wake
                    .wait_timeout_while(
                        ready.lock().unwrap(),
                        std::time::Duration::from_secs(5),
                        |ready| !*ready,
                    )
                    .unwrap();
                assert!(
                    !timeout.timed_out(),
                    "concurrent writer must release access reader"
                );
            }
        }
        CommandEvent::Succeeded(event) => {
            if let Ok(time) = event
                .reply
                .get_document("cursor")
                .and_then(|cursor| cursor.get_timestamp("atClusterTime"))
            {
                trace.snapshot_times.lock().unwrap().push(time);
            }
        }
        _ => {}
    }));
    Client::with_options(options).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn access_snapshot_session_uses_two_commands_at_one_time_for_every_target() {
    let (admin, name) = database("access_session_commands").await;
    let trace = Arc::new(Trace::default());
    let client = observed(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let original = store.resource_access_snapshot(&target).await.unwrap();
    let targets = [
        target,
        ResourceTarget::Worktree {
            worktree_id: original.lineage.worktree.as_ref().unwrap().id.clone(),
        },
        ResourceTarget::Workspace {
            workspace_id: original.lineage.workspace.as_ref().unwrap().id.clone(),
        },
        ResourceTarget::Sandbox {
            sandbox_id: original.lineage.sandbox.id.clone(),
        },
    ];
    for target in targets {
        let expected = store.resource_access_snapshot(&target).await.unwrap();
        for enabled in [false, true] {
            trace.commands.lock().unwrap().clear();
            trace.snapshot_times.lock().unwrap().clear();
            let reader = store.clone().with_access_snapshot_session(enabled);
            assert_eq!(
                reader.resource_access_snapshot(&target).await.unwrap(),
                expected
            );
            let commands = trace.commands.lock().unwrap().clone();
            assert_eq!(commands.len(), if enabled { 2 } else { 3 });
            assert_eq!(commands[0].0, "aggregate");
            assert_eq!(commands[1].0, "find");
            if enabled {
                let times = trace.snapshot_times.lock().unwrap();
                assert_eq!(times.len(), 2, "both replies report their snapshot time");
                assert_eq!(times[0], times[1]);
                assert_eq!(
                    commands[1]
                        .1
                        .get_document("readConcern")
                        .unwrap()
                        .get_timestamp("atClusterTime")
                        .unwrap(),
                    times[0]
                );
                for (_, command) in &commands {
                    assert_eq!(
                        command
                            .get_document("readConcern")
                            .unwrap()
                            .get_str("level")
                            .unwrap(),
                        "snapshot"
                    );
                    assert_eq!(
                        command.get_document("lsid").unwrap(),
                        commands[0].1.get_document("lsid").unwrap()
                    );
                    for field in ["startTransaction", "txnNumber", "autocommit"] {
                        assert!(
                            !command.contains_key(field),
                            "unexpected transaction field {field}"
                        );
                    }
                }
            } else {
                assert_eq!(commands[2].0, "abortTransaction");
                assert!(commands[0].1.get_bool("startTransaction").unwrap());
            }
        }
    }
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn access_snapshot_session_keeps_lineage_and_shares_before_concurrent_revocation() {
    let (admin, name) = database("access_session_revocation").await;
    let trace = Arc::new(Trace::default());
    let client = observed(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let writer = MongoSandboxStore::open(&admin, &name).await.unwrap();
    for enabled in [false, true] {
        // Enabling read sessions must not change share writers' transactions.
        let writer = writer.clone().with_access_snapshot_session(enabled);
        let before = writer.resource_access_snapshot(&target).await.unwrap();
        let key = format!("session-share-{enabled}");
        let receipt = writer
            .mutate_share(mutation(&before, &key, ShareMode::View))
            .await
            .unwrap();
        let expected = writer.resource_access_snapshot(&target).await.unwrap();
        let reader = store.clone().with_access_snapshot_session(enabled);
        *trace.resume.0.lock().unwrap() = false;
        trace.pause_shares.store(true, Ordering::SeqCst);
        let read_target = target.clone();
        let task = tokio::spawn(async move { reader.resource_access_snapshot(&read_target).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), trace.entered.notified())
            .await
            .unwrap();
        let mut revoke = mutation(&expected, &format!("revoke-{enabled}"), ShareMode::View);
        revoke.change = ShareChange::Revoke {
            share_id: receipt.share.id.clone(),
        };
        revoke.expected_share_revision = Some(receipt.share.revision);
        writer.mutate_share(revoke).await.unwrap();
        *trace.resume.0.lock().unwrap() = true;
        trace.resume.1.notify_one();
        assert_eq!(task.await.unwrap().unwrap(), expected);
        assert!(store
            .clone()
            .with_access_snapshot_session(enabled)
            .resource_access_snapshot(&target)
            .await
            .unwrap()
            .shares
            .is_empty());
    }
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn access_snapshot_session_preserves_missing_corrupt_and_full_agent_validation() {
    let (client, name) = database("access_session_validation").await;
    let (store, target) = fixture(&client, &name).await;
    for enabled in [false, true] {
        let reader = store.clone().with_access_snapshot_session(enabled);
        assert_eq!(
            reader
                .resource_access_snapshot(&ResourceTarget::Sandbox {
                    sandbox_id: SandboxId("sbx_missing".into()),
                })
                .await,
            Err(ResourceAccessStoreError::NotFound)
        );
    }
    store
        .agents
        .update_one(doc! {}, doc! {"$set":{"record.conversation.active_turn":1}})
        .await
        .unwrap();
    for enabled in [false, true] {
        assert_eq!(
            store
                .clone()
                .with_access_snapshot_session(enabled)
                .resource_access_snapshot(&target)
                .await,
            Err(ResourceAccessStoreError::Corrupt)
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn access_snapshot_session_preserves_timeout_and_transient_retry_budget() {
    let (admin, name) = database("access_session_failures").await;
    let trace = Arc::new(Trace::default());
    let client = observed(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    for enabled in [false, true] {
        let reader = store.clone().with_access_snapshot_session(enabled);
        for (failures, code, transient, succeeds) in [
            (1, 50, false, false),
            (1, 112, true, true),
            (TRANSACTION_ATTEMPTS, 112, true, false),
        ] {
            let mut data = doc! {"failCommands":["aggregate"], "errorCode":code};
            if transient {
                data.insert("errorLabels", vec!["TransientTransactionError"]);
            }
            trace.commands.lock().unwrap().clear();
            admin.database("admin").run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":failures as i32}, "data":data,
            }).await.unwrap();
            let result = reader.resource_access_snapshot(&target).await;
            admin
                .database("admin")
                .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
                .await
                .unwrap();
            if succeeds {
                assert!(result.is_ok());
            } else {
                assert_eq!(result, Err(ResourceAccessStoreError::Unavailable));
            }
            let aggregates = trace
                .commands
                .lock()
                .unwrap()
                .iter()
                .filter(|(name, _)| name == "aggregate")
                .count();
            assert_eq!(aggregates, failures + usize::from(succeeds));
            assert!(reader.resource_access_snapshot(&target).await.is_ok());
        }
    }
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}
