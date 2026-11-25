use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn generic_transaction_receipts_reconcile_from_one_commit_time_snapshot() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("test requires a failpoint-enabled MongoDB replica set");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-transaction-reconcile-{suffix}");
    let name = format!("cma_transaction_reconcile_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Vec::<(String, Document)>::new()));
    let observed = Arc::clone(&commands);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_transaction_reconcile");
    store.create_sandbox(parent.clone()).await.unwrap();
    let (worktree, agent) = worktree_and_agent(&parent.id);
    let mut workspace = workspace(&parent, worktree.workspace_id.as_str(), "project");
    store.create_workspace(workspace.clone()).await.unwrap();
    workspace.status = WorkspaceStatus::Ready;
    workspace.resolved_commit = Some("a".repeat(40));
    workspace.ready_at = Some(MINUTE.into());
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, MINUTE.into());
    workspace.revision += 1;
    parent.begin_retirement(MINUTE.into()).unwrap();
    let transactions = [
        SandboxTransaction::CreateWorktreeAndAgent {
            worktree: Box::new(worktree.clone()),
            agent: Box::new(agent.clone()),
            event_at: MINUTE.into(),
            event: SandboxEventKind::AgentCreated {
                workspace_id: workspace.id.clone(),
                worktree_id: worktree.id.clone(),
                agent_id: agent.id.clone(),
            },
        },
        SandboxTransaction::SaveWorkspaceAndAppend {
            repository_runtime: None,
            expected_revision: 1,
            workspace: Box::new(workspace.clone()),
            event_at: MINUTE.into(),
            event: SandboxEventKind::WorkspaceReady {
                workspace_id: workspace.id.clone(),
                resolved_commit: workspace.resolved_commit.clone().unwrap(),
            },
        },
        SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision: 1,
            sandbox: Box::new(parent.clone()),
            event_at: MINUTE.into(),
            event: SandboxEventKind::StatusChanged {
                from: SandboxStatus::Active,
                to: SandboxStatus::Retiring,
            },
        },
    ];
    for (index, transaction) in transactions.into_iter().enumerate() {
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
                        "errmsg": "injected ambiguous aggregate transaction",
                    },
                },
            })
            .await
            .unwrap();
        let result = store.commit(transaction).await;
        admin
            .database("admin")
            .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
            .await
            .expect("disable failpoint before assertions");
        result.unwrap();
        assert_pinned_snapshot(&commands.lock().unwrap(), if index == 0 { 3 } else { 2 });
        let events = store.sandbox_events(&parent.id, 0, 10).await.unwrap();
        assert_eq!(events.len(), index + 1);
        assert_eq!(events.last().unwrap().seq, (index + 1) as u64);
    }
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree)
    );
    client.database(&name).drop().await.unwrap();
}

fn assert_pinned_snapshot(commands: &[(String, Document)], expected_reads: usize) {
    let commits = commands
        .iter()
        .enumerate()
        .filter_map(|(index, (name, _))| (name == "commitTransaction").then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(commits.len(), 5);
    let reads = commands
        .iter()
        .skip(commits.last().unwrap() + 1)
        .filter_map(|(name, command)| (name == "find").then_some(command))
        .collect::<Vec<_>>();
    assert_eq!(reads.len(), expected_reads);
    let first = reads[0].get_document("readConcern").unwrap();
    let snapshot_time = first.get_timestamp("atClusterTime").unwrap();
    let session = reads[0].get_document("lsid").unwrap();
    for read in reads {
        let concern = read.get_document("readConcern").unwrap();
        assert_eq!(concern.get_str("level").unwrap(), "snapshot");
        assert_eq!(
            concern.get_timestamp("atClusterTime").unwrap(),
            snapshot_time
        );
        assert_eq!(read.get_document("lsid").unwrap(), session);
    }
}

#[path = "../agent_commands/reserve/ownership_tests/typed_fence.rs"]
mod admission_leaf_fence;
#[path = "../agent_commands/typed_fence_tests.rs"]
pub(in crate::store) mod command_fence;
#[path = "../work_publication/authority_mongo_tests.rs"]
mod work_authority;
