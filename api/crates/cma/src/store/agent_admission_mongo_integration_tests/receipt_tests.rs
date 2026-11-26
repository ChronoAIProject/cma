use super::*;

#[path = "receipt_tests/legacy_reuse_body.rs"]
mod legacy_reuse_body;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_replays_reject_an_incomplete_log() {
    let (client, database, store, _, _, admission) = fixture("replay_orphan").await;
    let creation = match store.admit_agent(admission.clone()).await.unwrap() {
        AgentAdmissionResult::Created(creation) => creation,
        AgentAdmissionResult::Existing(_) => panic!("fresh admission unexpectedly replayed"),
    };
    store
        .agent_log
        .insert_one(log_document(&orphan(&admission, 2)).unwrap())
        .await
        .unwrap();

    assert!(matches!(
        store.admit_agent(admission.clone()).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_quarantined(&store, &admission.agent_id).await;
    let mut anchor_session = store.client.start_session().await.unwrap();
    store
        .agents
        .find_one(doc! { "_id": admission.agent_id.as_str() })
        .session(&mut anchor_session)
        .await
        .unwrap();
    let snapshot_time = anchor_session.operation_time().unwrap();
    assert!(matches!(
        replay_after_duplicate(&store, &admission, snapshot_time).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(
        store.get_agent(&admission.agent_id).await.unwrap(),
        Some(creation.agent)
    );

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_reconciliation_rejects_an_above_high_water_orphan() {
    let (client, database, store, _, _, admission) = fixture("reconcile_orphan").await;
    let mut transaction = store.client.start_session().await.unwrap();
    let mut scope = crate::store::agent_log_segments::transaction_scope::TransactionScope::start(
        &mut transaction,
    )
    .await
    .unwrap();
    let receipt =
        match super::super::apply_admission(&store, &admission, false, None, &mut scope).await {
            Ok(AdmissionReceipt::Created(receipt)) => receipt,
            Ok(AdmissionReceipt::Existing(_) | AdmissionReceipt::Reused(_)) => {
                panic!("fresh admission unexpectedly replayed")
            }
            Err(_) => panic!("fresh admission transaction failed"),
        };
    scope.release().commit_transaction().await.unwrap();

    let mut anchor_session = store.client.start_session().await.unwrap();
    store
        .agent_log
        .insert_one(log_document(&orphan(&admission, 2)).unwrap())
        .session(&mut anchor_session)
        .await
        .unwrap();
    let snapshot_time = anchor_session.operation_time().unwrap();
    assert!(matches!(
        reconcile_created(&store, &receipt, snapshot_time).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_quarantined(&store, &admission.agent_id).await;

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn admission_replay_majority_commits_and_retries_an_ambiguous_commit() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let suffix = format!("replay_commit_{}", uuid::Uuid::new_v4().simple());
    let app_name = format!("cma-agent-admission-{suffix}");
    let database = format!("cma_811_adm_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Vec::<(String, Document)>::new()));
    let observed_commands = Arc::clone(&commands);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed_commands
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let parent = sandbox(&suffix);
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = workspace(&parent, &suffix);
    store.create_workspace(workspace.clone()).await.unwrap();
    let admission = admission(&workspace, &suffix);
    let created = match store.admit_agent(admission.clone()).await.unwrap() {
        AgentAdmissionResult::Created(creation) => creation,
        AgentAdmissionResult::Existing(_) => panic!("fresh admission unexpectedly replayed"),
    };
    commands.lock().unwrap().clear();

    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": &app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous replay commit",
                },
            },
        })
        .await
        .unwrap();
    let outcome = store.admit_agent(admission).await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("commit failpoint must be disabled before assertions");

    assert_eq!(outcome.unwrap(), AgentAdmissionResult::Existing(created));
    let commands = commands.lock().unwrap().clone();
    let commits = commands
        .iter()
        .filter_map(|(name, command)| (name == "commitTransaction").then_some(command))
        .collect::<Vec<_>>();
    // The replay transaction spends its bounded commit attempts. An unknown
    // outcome then reconciles the exact receipt read-only at the recorded
    // snapshot and never starts another transaction (#3112).
    assert_eq!(commits.len(), 5);
    assert!(commits.iter().all(|command| {
        command
            .get_document("writeConcern")
            .ok()
            .and_then(|concern| concern.get_str("w").ok())
            == Some("majority")
    }));
    let first_transaction = commits[0].get("txnNumber").unwrap();
    assert!(commits
        .iter()
        .all(|command| command.get("txnNumber") == Some(first_transaction)));
    let last_commit = commands
        .iter()
        .rposition(|(name, _)| name == "commitTransaction")
        .unwrap();
    assert!(commands[last_commit + 1..].iter().all(|(_, command)| {
        command.get("txnNumber").is_none() && !command.contains_key("startTransaction")
    }));
    assert!(!commands.iter().any(|(name, _)| name == "abortTransaction"));

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn admission_reconciles_every_artifact_at_the_ambiguous_commit_time() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let suffix = format!("ambiguous_{}", uuid::Uuid::new_v4().simple());
    let app_name = format!("cma-agent-admission-{suffix}");
    let database = format!("cma_811_adm_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Vec::<(String, Document)>::new()));
    let observed_commands = Arc::clone(&commands);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed_commands
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let parent = sandbox(&suffix);
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = workspace(&parent, &suffix);
    store.create_workspace(workspace.clone()).await.unwrap();
    let admission = admission(&workspace, &suffix);
    commands.lock().unwrap().clear();

    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": &app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous commit",
                },
            },
        })
        .await
        .unwrap();
    let outcome = store.admit_agent(admission.clone()).await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("commit failpoint must be disabled before assertions");

    let creation = match outcome.unwrap() {
        AgentAdmissionResult::Created(creation) => creation,
        AgentAdmissionResult::Existing(_) => panic!("fresh admission unexpectedly replayed"),
    };
    let commands = commands.lock().unwrap().clone();
    let commit_positions = commands
        .iter()
        .enumerate()
        .filter_map(|(position, (name, _))| (name == "commitTransaction").then_some(position))
        .collect::<Vec<_>>();
    assert_eq!(commit_positions.len(), 5);
    let reconciliation_finds = commands
        .iter()
        .skip(commit_positions.last().copied().unwrap() + 1)
        .filter_map(|(name, command)| (name == "find").then_some(command))
        .collect::<Vec<_>>();
    assert_eq!(
        reconciliation_finds
            .iter()
            .map(|command| command.get_str("find").unwrap())
            .collect::<Vec<_>>(),
        vec![
            "workspaces",
            "sandboxes",
            "worktrees",
            "agents",
            "trigger_first_message_bindings",
            "agent_creation_provenance",
            "agents",    // Validate the same-snapshot integrity envelope.
            "agent_log", // Exact retained tail.
            "agent_log", // Probe beyond the trusted head.
            "agent_log", // Complete multi-record receipt remains mandatory.
            "sandbox_events",
            "sandbox_events",
        ]
    );
    let snapshot_time = reconciliation_finds[0]
        .get_document("readConcern")
        .unwrap()
        .get_timestamp("atClusterTime")
        .unwrap();
    assert!(reconciliation_finds.iter().all(|command| {
        command
            .get_document("readConcern")
            .ok()
            .is_some_and(|concern| {
                concern.get_str("level").ok() == Some("snapshot")
                    && concern.get_timestamp("atClusterTime").ok() == Some(snapshot_time)
            })
    }));
    assert_eq!(
        store.get_agent(&creation.agent.id).await.unwrap(),
        Some(creation.agent)
    );
    assert_eq!(
        store.get_worktree(&creation.worktree.id).await.unwrap(),
        Some(creation.worktree)
    );

    client.database(&database).drop().await.unwrap();
}

async fn assert_quarantined(store: &MongoSandboxStore, id: &AgentId) {
    assert!(store
        .agents
        .find_one(doc! { "_id": id.as_str() })
        .await
        .unwrap()
        .unwrap()
        .contains_key(crate::store::agent_log_integrity::QUARANTINE_FIELD));
}
