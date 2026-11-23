//! Transaction outcomes for the channel history writer against disposable MongoDB.
use super::*;
use mongodb::{bson::Timestamp, options::ReadConcern};

#[derive(Clone, Copy)]
enum CommitFault {
    UnknownUnapplied,
    UnknownApplied,
    UnknownAppliedUnreadable,
    ConfirmedAbort,
}

async fn channel_commit(fault: CommitFault) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let db = format!("cma_channel_commit_{suffix}");
    let app = format!("cma-channel-commit-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&commands);
    let proof_injected = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let injected = Arc::clone(&proof_injected);
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observe_now = Arc::clone(&armed);
    let commit_time = Arc::new(Mutex::new(None::<Timestamp>));
    let observed_commit = Arc::clone(&commit_time);
    let control_uri = uri.clone();
    let control_app = app.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if !observe_now.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        match event {
            CommandEvent::Succeeded(event)
                if event.command_name == "commitTransaction"
                    && event.reply.contains_key("writeConcernError") =>
            {
                if let Ok(operation_time) = event.reply.get_timestamp("operationTime") {
                    *observed_commit.lock().unwrap() = Some(operation_time);
                }
            }
            CommandEvent::Started(event) => {
                let mut observed = observed.lock().unwrap();
                let follows_commit = event.command_name == "find"
                    && observed.iter().any(|(name, _)| name == "commitTransaction");
                observed.push((event.command_name, event.command));
                drop(observed);
                if matches!(fault, CommitFault::UnknownAppliedUnreadable)
                    && follows_commit
                    && !injected.swap(true, std::sync::atomic::Ordering::SeqCst)
                {
                    fail_proof_read(control_uri.clone(), control_app.clone());
                }
            }
            _ => {}
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &db).await.unwrap();
    let parent = sandbox(&suffix);
    store.create_sandbox(parent.clone()).await.unwrap();
    let ws = workspace(&parent, &suffix);
    store.create_workspace(ws.clone()).await.unwrap();
    let creation = store.admit_agent(admission(&ws, &suffix)).await.unwrap();
    let agent = &creation.creation().agent.id;
    let before = store.agent_channel_usage_snapshot(agent).await.unwrap();
    let request = mutation(
        &store,
        agent,
        ChannelUsageChange::Attach {
            binding: binding("binding-one", 1, None),
        },
        ChannelBindingChangeReason::Attachment,
        "commit-fault",
    )
    .await;

    let mut data = doc! {"appName": app, "failCommands": ["commitTransaction"]};
    let mode = match fault {
        CommitFault::UnknownUnapplied => {
            data.insert("errorCode", 1_i32);
            data.insert("errorLabels", ["UnknownTransactionCommitResult"].to_vec());
            mongodb::bson::Bson::String("alwaysOn".into())
        }
        CommitFault::UnknownApplied | CommitFault::UnknownAppliedUnreadable => {
            data.insert("writeConcernError", doc! {
                "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "synthetic unknown commit",
            });
            mongodb::bson::Bson::String("alwaysOn".into())
        }
        CommitFault::ConfirmedAbort => {
            // Reject a body update before any commit so the server aborts the transaction.
            data.insert("failCommands", ["update"].to_vec());
            data.insert("errorCode", 112_i32);
            data.insert("errorLabels", ["TransientTransactionError"].to_vec());
            mongodb::bson::Bson::Document(doc! {"times": 1_i32})
        }
    };
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": mode, "data": data,
        })
        .await
        .unwrap();
    commands.lock().unwrap().clear();

    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let outcome = store.record_agent_channel_usage(request.clone()).await;
    armed.store(false, std::sync::atomic::Ordering::SeqCst);
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": "off",
        })
        .await
        .unwrap();
    let trace = commands.lock().unwrap().clone();
    if matches!(
        fault,
        CommitFault::UnknownApplied | CommitFault::UnknownAppliedUnreadable
    ) {
        let at = commit_time
            .lock()
            .unwrap()
            .expect("ambiguous commit reply operationTime");
        let mut proof = client
            .start_session()
            .snapshot(true)
            .snapshot_time(at)
            .await
            .unwrap();
        assert!(
            store
                .channel_usage_events
                .find_one(doc! {"_id": request.operation_id()})
                .read_concern(ReadConcern::snapshot())
                .max_time(std::time::Duration::from_secs(5))
                .session(&mut proof)
                .await
                .unwrap()
                .is_some(),
            "independent majority snapshot must see the applied event"
        );
    }
    let rows = store
        .channel_usage_events
        .count_documents(doc! {})
        .await
        .unwrap();
    let current = store.agent_channel_usage_snapshot(agent).await.unwrap();
    let writes = trace
        .iter()
        .filter(|(name, cmd)| {
            name == "insert"
                && cmd.get_str("insert").ok() == Some(store.channel_usage_events.name())
        })
        .count();
    let transactions = trace
        .iter()
        .filter(|(_, cmd)| cmd.get_bool("startTransaction").ok() == Some(true))
        .count();
    client.database(&db).drop().await.unwrap();

    match fault {
        CommitFault::UnknownUnapplied => {
            assert!(matches!(outcome, Err(SandboxStoreError::Unavailable(_))));
            assert_eq!(current, before);
            assert_eq!(rows, 0);
            assert_eq!(
                transactions, 1,
                "uncertain commit must not start another writer"
            );
            assert_eq!(writes, 1);
        }
        CommitFault::UnknownAppliedUnreadable => {
            assert!(proof_injected.load(std::sync::atomic::Ordering::SeqCst));
            assert!(matches!(outcome, Err(SandboxStoreError::Unavailable(_))));
            assert_eq!(rows, 1);
            assert_eq!(current.state.revision, 1);
            assert_eq!(writes, 1);
            assert_eq!(
                transactions, 1,
                "an unavailable proof must not re-open the writer"
            );
        }
        CommitFault::UnknownApplied | CommitFault::ConfirmedAbort => {
            let event = outcome.unwrap();
            assert_eq!(event.operation_id, request.operation_id());
            assert_eq!(event.at, request.at);
            assert_eq!(event.after, current.state);
            assert_eq!(rows, 1);
            assert_eq!(
                transactions,
                if matches!(fault, CommitFault::ConfirmedAbort) {
                    2
                } else {
                    1
                }
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn channel_commit_unknown_unapplied_starts_no_second_writer() {
    channel_commit(CommitFault::UnknownUnapplied).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn channel_commit_unknown_applied_proves_one_exact_event() {
    channel_commit(CommitFault::UnknownApplied).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn channel_commit_confirmed_abort_retries_original_event() {
    channel_commit(CommitFault::ConfirmedAbort).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn channel_commit_unknown_applied_unreadable_proof_does_not_replay() {
    channel_commit(CommitFault::UnknownAppliedUnreadable).await;
}

fn fail_proof_read(uri: String, app: String) {
    // The command observer must arm the fault before Mongo receives the read.
    // Drive the separate control client without blocking its own runtime.
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    let control = Client::with_uri_str(uri).await.unwrap();
                    control
                        .database("admin")
                        .run_command(doc! {
                            "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
                            "data": {"appName": app, "failCommands": ["find"], "errorCode": 1_i32},
                        })
                        .await
                        .unwrap();
                    control.shutdown().await;
                })
                .await
                .unwrap();
            });
    })
    .join()
    .unwrap();
}
