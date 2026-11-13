use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn agent_control_reconciles_a_committed_but_ambiguous_transaction() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let app_name = format!(
        "cma-agent-control-ambiguous-{}-{sequence}",
        std::process::id()
    );
    let name = format!(
        "cma_811_agent_control_ambiguous_{}_{sequence}",
        std::process::id()
    );
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let trace = Arc::new(Mutex::new(CommandTrace::default()));
    let observed_trace = Arc::clone(&trace);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut trace = observed_trace.lock().unwrap();
        match event {
            CommandEvent::Started(event) => {
                trace.started.push((event.command_name, event.command));
            }
            CommandEvent::Failed(event) if event.command_name == "commitTransaction" => {
                trace.failed_commits += 1;
            }
            CommandEvent::Succeeded(event) if event.command_name == "commitTransaction" => {
                trace.commit_replies.push(event.reply);
            }
            _ => {}
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let suffix = format!("ambiguous_{sequence}");
    let (parent, worktree, claimed) = claimed_agent_work_fixture(&store, &suffix).await;
    let workspace = store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let request = AgentStopRequest {
        target: AgentControlTarget::from(&claimed),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let log_count_before = store
        .agent_log
        .count_documents(doc! { "agent_id": claimed.id.as_str() })
        .await
        .unwrap();
    let event_count_before = store
        .sandbox_events
        .count_documents(Document::new())
        .await
        .unwrap();
    *trace.lock().unwrap() = CommandTrace::default();

    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous commit",
                },
            },
        })
        .await
        .unwrap();
    let outcome = store.schedule_agent_stop(request.clone()).await;
    let disable_result = admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await;
    disable_result.expect("commit failpoint must be disabled before assertions");

    let scheduled = match outcome.unwrap() {
        AgentControlResult::Scheduled(agent) => agent,
        AgentControlResult::Existing(_) => panic!("fresh stop unexpectedly replayed"),
    };
    let trace = trace.lock().unwrap().clone();
    let commits = trace
        .started
        .iter()
        .filter_map(|(name, command)| (name == "commitTransaction").then_some(command))
        .collect::<Vec<_>>();
    assert_eq!(commits.len(), 5);
    assert_eq!(trace.failed_commits, 0);
    assert_eq!(trace.commit_replies.len(), 5);
    assert!(trace.commit_replies.iter().all(|reply| {
        reply
            .get_document("writeConcernError")
            .ok()
            .and_then(|error| error.get_i32("code").ok())
            == Some(50)
    }));
    let lsid = commits[0].get_document("lsid").unwrap();
    let transaction_number = commits[0].get("txnNumber").unwrap();
    assert!(commits.iter().all(|command| {
        command.get_document("lsid").ok() == Some(lsid)
            && command.get("txnNumber") == Some(transaction_number)
            && command.get_bool("autocommit").ok() == Some(false)
    }));
    let last_commit_position = trace
        .started
        .iter()
        .rposition(|(name, _)| name == "commitTransaction")
        .unwrap();
    let reconciliation_reads = trace
        .started
        .iter()
        .skip(last_commit_position + 1)
        .filter_map(|(name, command)| (name == "find").then_some(command))
        .collect::<Vec<_>>();
    assert_eq!(reconciliation_reads.len(), 4);
    let reconciliation_lsid = reconciliation_reads[0].get_document("lsid").unwrap();
    assert_ne!(reconciliation_lsid, lsid);
    let snapshot_time = reconciliation_reads[0]
        .get_document("readConcern")
        .unwrap()
        .get_timestamp("atClusterTime")
        .unwrap();
    assert!(reconciliation_reads.iter().all(|command| {
        command.get_document("lsid").ok() == Some(reconciliation_lsid)
            && command_uses_snapshot_read(command)
            && command
                .get_document("readConcern")
                .ok()
                .and_then(|concern| concern.get_timestamp("atClusterTime").ok())
                == Some(snapshot_time)
    }));

    assert_eq!(scheduled.revision, claimed.revision + 1);
    assert_eq!(
        scheduled.conversation.last_seq,
        claimed.conversation.last_seq
    );
    assert_eq!(
        store.get_agent(&scheduled.id).await.unwrap(),
        Some(scheduled.clone())
    );
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace)
    );
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree)
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": scheduled.id.as_str() })
            .await
            .unwrap(),
        log_count_before
    );
    assert_eq!(
        store
            .sandbox_events
            .count_documents(Document::new())
            .await
            .unwrap(),
        event_count_before
    );

    let agent_document = store
        .agents
        .find_one(doc! { "_id": scheduled.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        agent_document.get_i64("revision").unwrap(),
        i64::try_from(scheduled.revision).unwrap()
    );
    let receipt = agent_document.get_document(STOP_RECEIPT_FIELD).unwrap();
    assert_eq!(
        receipt.get_i64("document_schema_version").unwrap(),
        DOCUMENT_SCHEMA_VERSION
    );
    assert_eq!(
        receipt.get_i64("scheduled_revision").unwrap(),
        i64::try_from(scheduled.revision).unwrap()
    );
    assert_eq!(
        receipt.get("request"),
        Some(&mongodb::bson::serialize_to_bson(&ControlRequest::Stop(request.clone())).unwrap())
    );
    let operation_id = receipt.get_str("operation_id").unwrap().to_owned();
    assert!(operation_id.starts_with("ctl_") && operation_id.len() == 36);
    for document in [
        store
            .worktrees
            .find_one(doc! { "_id": scheduled.worktree_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        store
            .workspaces
            .find_one(doc! { "_id": scheduled.workspace_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        store
            .sandboxes
            .find_one(doc! { "_id": scheduled.sandbox_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
    ] {
        assert_eq!(
            document.get_str("last_agent_control_operation").unwrap(),
            operation_id.as_str()
        );
    }
    assert_eq!(
        store
            .agents
            .count_documents(doc! { "_id": scheduled.id.as_str() })
            .await
            .unwrap(),
        1
    );

    assert_eq!(
        store.schedule_agent_stop(request.clone()).await.unwrap(),
        AgentControlResult::Existing(scheduled.clone())
    );
    let mut changed = request;
    changed.reason = AgentStopReason::Operator;
    assert_eq!(
        store.schedule_agent_stop(changed).await,
        Err(AgentControlStoreError::StopReasonConflict)
    );
    assert_eq!(
        store
            .agents
            .find_one(doc! { "_id": claimed.id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        agent_document
    );

    client.database(&name).drop().await.unwrap();
}
