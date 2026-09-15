use super::*;
use crate::workspace_journal::{self, Begin as WorkspaceBegin};
use layer3_adapter::WorkspaceRemoveReceipt;

async fn prepare_child(config: &RuntimeConfig, supervisor: &Supervisor, name: &str) -> AgentLaunch {
    let workspace_id = format!("wks_{name}");
    let prepared = supervisor
        .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
            format!("workspace:{workspace_id}:prepare:v1"),
            &workspace_id,
            name,
            WorkspaceSource::Init,
        )))
        .await;
    let ResponseFrame::PrepareWorkspace(workspace) = prepared else {
        panic!("{prepared:?}")
    };
    let agent_id = format!("agent-{name}");
    let added = supervisor
        .dispatch(add_worktree_request(WorktreeRequest::new(
            format!("worktree:{name}:{agent_id}:add:v1"),
            name,
            &agent_id,
            workspace.resolved_commit,
        )))
        .await;
    let ResponseFrame::AddWorktree(worktree) = added else {
        panic!("{added:?}")
    };
    let materialized = supervisor
        .dispatch(materialize_home_request(home_plan(&agent_id, b"profile")))
        .await;
    assert!(
        matches!(materialized, ResponseFrame::MaterializeAgentHome(_)),
        "{materialized:?}"
    );
    let mut request = launch(
        config,
        &worktree.receipt,
        "printf live > started; while IFS= read -r line; do :; done",
    );
    request.operation_id = format!("agent:{agent_id}:open:v1");
    request.home = config.agent_home(&agent_id).to_string_lossy().into_owned();
    request.agent_id = AgentSlot::new(agent_id);
    std::fs::write(PathBuf::from(&request.cwd).join("sentinel"), name).unwrap();
    request
}

async fn open(supervisor: &Supervisor, launch: &AgentLaunch) {
    let response = supervisor
        .dispatch(open_agent_request(launch.clone()))
        .await;
    assert!(
        matches!(response, ResponseFrame::OpenAgent(_)),
        "{response:?}"
    );
    wait_for_file(&PathBuf::from(&launch.cwd).join("started")).await;
}

async fn close(supervisor: &Supervisor, launch: &AgentLaunch) {
    let response = supervisor
        .dispatch(close_agent_request(AgentCloseRequest::new(
            format!("agent:{}:close:v1", launch.agent_id.as_str()),
            launch.agent_id.clone(),
        )))
        .await;
    assert!(
        matches!(response, ResponseFrame::CloseAgent(_)),
        "{response:?}"
    );
}

fn removal() -> WorkspaceRemoveRequest {
    WorkspaceRemoveRequest::new("workspace:wks_target:remove:v1", "wks_target", "target")
}

fn remove_journal(
    config: &RuntimeConfig,
    request: &WorkspaceRemoveRequest,
) -> WorkspaceBegin<WorkspaceRemoveReceipt> {
    workspace_journal::begin(
        &config.state_root,
        &config.home,
        config.allow_local_git,
        &config.sandbox_id,
        &request.workspace_id,
        "remove",
        &request.operation_id,
        &request.request_sha256,
        request,
    )
    .unwrap()
}

async fn assert_live(supervisor: &Supervisor, launch: &AgentLaunch) {
    let agents = supervisor.agents.read().await;
    let process = agents.get(&launch.agent_id).unwrap();
    assert_eq!(process.launch, *launch);
    assert!(!process.bridge.is_ended());
    assert_eq!(
        std::fs::read(PathBuf::from(&launch.cwd).join("started")).unwrap(),
        b"live"
    );
}

#[tokio::test]
async fn workspace_remove_keeps_gate_from_idle_check_through_queued_open() {
    let config = RuntimeConfig::for_test(root("workspace-remove-open-race"));
    let supervisor = provision(&config).await;
    let target = prepare_child(&config, &supervisor, "target").await;
    let sibling = prepare_child(&config, &supervisor, "sibling").await;
    open(&supervisor, &sibling).await;

    // Pause exactly at the idle inventory read, after journal recovery. Before
    // the fix this point had already released operation_gate, allowing an open
    // to pass the check before the detached removal acquired the gate again.
    let inventory = supervisor.agents.write().await;
    let mut remove = Box::pin(supervisor.dispatch(RequestFrame::RemoveWorkspace(removal())));
    assert!(futures_util::poll!(remove.as_mut()).is_pending());
    assert!(
        supervisor.operation_gate.try_lock().is_err(),
        "Workspace idle validation must still own the mutation gate"
    );
    let mut queued_open = Box::pin(supervisor.dispatch(open_agent_request(target.clone())));
    assert!(futures_util::poll!(queued_open.as_mut()).is_pending());
    drop(inventory);
    let (removed, opened) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(remove, queued_open)
    })
    .await
    .expect("serialized removal and open converge");
    assert!(
        matches!(removed, ResponseFrame::RemoveWorkspace(_)),
        "{removed:?}"
    );
    assert_eq!(failure(opened).code, "agent_worktree_invalid");
    assert!(!config.home.join("target").exists());
    assert!(!supervisor
        .agents
        .read()
        .await
        .contains_key(&target.agent_id));
    assert_live(&supervisor, &sibling).await;
    assert_eq!(
        std::fs::read(PathBuf::from(&sibling.cwd).join("sentinel")).unwrap(),
        b"sibling"
    );
    close(&supervisor, &sibling).await;
    cleanup(&config);
}

#[tokio::test]
async fn workspace_remove_recovery_preserves_open_agents_until_exact_close() {
    for restart in [false, true] {
        let config = RuntimeConfig::for_test(root(&format!("workspace-remove-recovery-{restart}")));
        let mut supervisor = provision(&config).await;
        let target = prepare_child(&config, &supervisor, "target").await;
        let sibling = prepare_child(&config, &supervisor, "sibling").await;
        open(&supervisor, &target).await;
        open(&supervisor, &sibling).await;
        let request = removal();
        assert!(matches!(
            remove_journal(&config, &request),
            WorkspaceBegin::New(_)
        ));

        if restart {
            // Simulate process loss without close receipts: recovery must also
            // honor durable opens when its in-memory Agent registry is empty.
            let bridges = supervisor
                .agents
                .read()
                .await
                .values()
                .map(|process| Arc::clone(&process.bridge))
                .collect::<Vec<_>>();
            for bridge in bridges {
                bridge.terminate().await.unwrap();
            }
            drop(supervisor);
            supervisor = recreate(&config).await;
            assert!(supervisor.agents.read().await.is_empty());
        } else {
            // Recovery itself must acquire the same gate before reading idle
            // inventory, and leave it held while that read is suspended.
            let inventory = supervisor.agents.write().await;
            let mut recovery = Box::pin(supervisor.recover_journals());
            assert!(futures_util::poll!(recovery.as_mut()).is_pending());
            assert!(supervisor.operation_gate.try_lock().is_err());
            drop(inventory);
            recovery.await.unwrap();
        }

        let refused = supervisor
            .dispatch(RequestFrame::RemoveWorkspace(request.clone()))
            .await;
        assert_eq!(failure(refused).code, "workspace_agent_open");
        assert!(matches!(
            remove_journal(&config, &request),
            WorkspaceBegin::Started(_)
        ));
        assert_eq!(
            std::fs::read(PathBuf::from(&target.cwd).join("sentinel")).unwrap(),
            b"target"
        );
        assert_eq!(mutation_journal::active_agents(&config).unwrap().len(), 2);
        if !restart {
            assert_live(&supervisor, &target).await;
            assert_live(&supervisor, &sibling).await;
        }

        // A held removal must not block the close that makes recovery safe.
        close(&supervisor, &target).await;
        supervisor.recover_journals().await.unwrap();
        assert!(!config.home.join("target").exists());
        let WorkspaceBegin::Completed(receipt) = remove_journal(&config, &request) else {
            panic!("recovery did not preserve and complete the original removal")
        };
        assert_eq!(receipt.operation_id, request.operation_id);
        assert_eq!(receipt.request_sha256, request.request_sha256);
        assert_eq!(receipt.workspace_id, request.workspace_id);
        let active = mutation_journal::active_agents(&config).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].agent_id, sibling.agent_id.as_str());
        assert_eq!(
            std::fs::read(PathBuf::from(&sibling.cwd).join("sentinel")).unwrap(),
            b"sibling"
        );
        if !restart {
            assert_live(&supervisor, &sibling).await;
        }
        close(&supervisor, &sibling).await;
        cleanup(&config);
    }
}

#[tokio::test]
async fn detached_workspace_remove_retains_transferred_gate_after_caller_cancellation() {
    let config = RuntimeConfig::for_test(root("workspace-remove-detached-gate"));
    let supervisor = provision(&config).await;
    prepare_child(&config, &supervisor, "target").await;
    let gate = Arc::clone(&supervisor.operation_gate).lock_owned().await;
    supervisor.ensure_workspace_idle("target").await.unwrap();
    let checkpoint = supervisor.held_checkpoint.lock().await;
    let request = removal();
    let mut dispatch = Box::pin(crate::workspace_runtime::dispatch(
        config.clone(),
        gate,
        Arc::clone(&supervisor.held_checkpoint),
        RequestFrame::RemoveWorkspace(request.clone()),
    ));
    assert!(futures_util::poll!(dispatch.as_mut()).is_pending());
    drop(dispatch);
    assert!(supervisor.operation_gate.try_lock().is_err());
    assert!(config.home.join("target").exists());
    drop(checkpoint);
    let _completed = tokio::time::timeout(Duration::from_secs(3), supervisor.operation_gate.lock())
        .await
        .expect("detached mutation releases the gate after completion");
    assert!(!config.home.join("target").exists());
    assert!(matches!(
        remove_journal(&config, &request),
        WorkspaceBegin::Completed(_)
    ));
    cleanup(&config);
}
