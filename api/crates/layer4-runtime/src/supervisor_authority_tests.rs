use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use layer3_adapter::wire::{FencedRequest, RequestFrame, ResponseFrame, WireFailure};
use layer3_adapter::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch, AgentSlot,
    CredentialFrame, InstallRequest, RuntimeCredential, WorkspaceRequest, WorkspaceSource,
    WorktreeReceipt, WorktreeRef, WorktreeRequest,
};

use super::*;
use crate::mutation_journal::{self, Begin, Operation};

fn root(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "cma-supervisor-authority-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn cleanup(config: &RuntimeConfig) {
    let _ = std::fs::remove_dir_all(&config.home);
    let _ = std::fs::remove_dir_all(&config.checkpoint_root);
    let _ = std::fs::remove_dir_all(&config.state_root);
}

fn fenced<T>(authority_fence: u64, request: T) -> FencedRequest<T> {
    FencedRequest::new(authority_fence, request)
}

fn failure(response: ResponseFrame) -> WireFailure {
    match response {
        ResponseFrame::Failure(failure) => failure,
        other => panic!("expected failure response, got {other:?}"),
    }
}

async fn provision(config: &RuntimeConfig) -> Arc<Supervisor> {
    let supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    let installed = supervisor
        .dispatch(RequestFrame::Install(InstallRequest::new(
            "install-authority-tests",
            Vec::new(),
        )))
        .await;
    assert!(
        matches!(installed, ResponseFrame::Install(_)),
        "{installed:?}"
    );
    credential(&supervisor).await;
    supervisor
}

async fn recreate(config: &RuntimeConfig) -> Arc<Supervisor> {
    let supervisor = Supervisor::start(config.clone(), SupervisorBoundary::default())
        .await
        .unwrap();
    credential(&supervisor).await;
    supervisor
}

async fn credential(supervisor: &Supervisor) {
    let response = supervisor
        .dispatch(RequestFrame::Credential(CredentialFrame {
            operation_id: "credential-authority-tests".into(),
            value: RuntimeCredential::new("test-credential"),
            custody_generation: Some(1),
            github_route: None,
        }))
        .await;
    assert!(
        matches!(response, ResponseFrame::Credential(_)),
        "{response:?}"
    );
}

async fn prepare_workspace(supervisor: &Supervisor, name: &str) -> String {
    let request = WorkspaceRequest::new(
        format!("workspace:wks_{name}:prepare:v1"),
        format!("wks_{name}"),
        name,
        WorkspaceSource::Init,
    );
    match supervisor
        .dispatch(RequestFrame::PrepareWorkspace(request))
        .await
    {
        ResponseFrame::PrepareWorkspace(receipt) => receipt.resolved_commit,
        other => panic!("workspace preparation failed: {other:?}"),
    }
}

fn home_plan(agent_id: &str) -> AgentHomePlan {
    AgentHomePlan {
        operation_id: format!("agent:{agent_id}:home:create:v1"),
        request_sha256: "a".repeat(64),
        agent_id: AgentSlot::new(agent_id),
        files: Vec::new(),
        symlinks: Vec::new(),
        skills: Vec::new(),
    }
}

fn launch(config: &RuntimeConfig, workspace: &str, agent_id: &str) -> AgentLaunch {
    AgentLaunch {
        operation_id: format!("agent:{agent_id}:open:v1"),
        request_sha256: "b".repeat(64),
        agent_id: AgentSlot::new(agent_id),
        cwd: crate::worktree::expected_path(&config.home, workspace, agent_id)
            .to_string_lossy()
            .into_owned(),
        home: config.agent_home(agent_id).to_string_lossy().into_owned(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "exit 0".into()],
        env: Default::default(),
    }
}

async fn assert_create_retired(
    supervisor: &Supervisor,
    authority_fence: u64,
    add: &WorktreeRequest,
    home: &AgentHomePlan,
    open: &AgentLaunch,
) {
    let responses = [
        supervisor
            .dispatch(RequestFrame::AddWorktree(fenced(
                authority_fence,
                add.clone(),
            )))
            .await,
        supervisor
            .dispatch(RequestFrame::MaterializeAgentHome(fenced(
                authority_fence,
                home.clone(),
            )))
            .await,
        supervisor
            .dispatch(RequestFrame::OpenAgent(fenced(
                authority_fence,
                open.clone(),
            )))
            .await,
    ];
    for response in responses {
        assert_eq!(failure(response).code, "mutation_authority_retired");
    }
}

#[tokio::test]
async fn cleanup_blocks_delayed_creates_for_every_resource_across_restart() {
    let config = RuntimeConfig::for_test(root("cleanup-dominates"));
    let supervisor = provision(&config).await;
    let workspace = "retired";
    let base = prepare_workspace(&supervisor, workspace).await;
    let agent_id = "agent-retired";
    let add = WorktreeRequest::new(
        format!("worktree:{workspace}:{agent_id}:add:v1"),
        workspace,
        agent_id,
        base,
    );
    let remove = WorktreeRef::new(
        format!("worktree:{workspace}:{agent_id}:remove:v1"),
        workspace,
        agent_id,
    );
    let home = home_plan(agent_id);
    let remove_home = AgentHomeRemoveRequest::new(
        format!("agent:{agent_id}:home:remove:v1"),
        AgentSlot::new(agent_id),
    );
    let open = launch(&config, workspace, agent_id);
    let close = AgentCloseRequest::new(
        format!("agent:{agent_id}:close:v1"),
        AgentSlot::new(agent_id),
    );

    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::RemoveWorktree(fenced(20, remove)))
            .await,
        ResponseFrame::RemoveWorktree(_)
    ));
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::RemoveAgentHome(fenced(20, remove_home)))
            .await,
        ResponseFrame::RemoveAgentHome(_)
    ));
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::CloseAgent(fenced(20, close)))
            .await,
        ResponseFrame::CloseAgent(_)
    ));

    assert_create_retired(&supervisor, 19, &add, &home, &open).await;
    assert_create_retired(&supervisor, 20, &add, &home, &open).await;
    assert!(!crate::worktree::expected_path(&config.home, workspace, agent_id).exists());
    assert!(!config.agent_home(agent_id).exists());
    assert!(supervisor.bridge(agent_id).await.is_none());
    drop(supervisor);

    let supervisor = recreate(&config).await;
    assert_create_retired(&supervisor, 19, &add, &home, &open).await;
    assert_create_retired(&supervisor, 20, &add, &home, &open).await;
    assert!(!crate::worktree::expected_path(&config.home, workspace, agent_id).exists());
    assert!(!config.agent_home(agent_id).exists());
    assert!(supervisor.bridge(agent_id).await.is_none());
    cleanup(&config);
}

#[tokio::test]
async fn newer_cleanup_supersedes_a_started_create_before_restart_recovery() {
    let config = RuntimeConfig::for_test(root("supersede-started"));
    let supervisor = provision(&config).await;
    let workspace = "recovery";
    let base = prepare_workspace(&supervisor, workspace).await;
    let agent_id = "agent-stale";
    let add = WorktreeRequest::new(
        format!("worktree:{workspace}:{agent_id}:add:v1"),
        workspace,
        agent_id,
        base,
    );
    let add_operation = Operation::add_worktree(&add);
    crate::mutation_authority::admit(&config, 19, &add_operation).unwrap();
    assert!(matches!(
        mutation_journal::begin::<WorktreeReceipt>(&config, &add_operation).unwrap(),
        Begin::New(_)
    ));
    let remove = WorktreeRef::new(
        format!("worktree:{workspace}:{agent_id}:remove:v1"),
        workspace,
        agent_id,
    );
    crate::mutation_authority::admit(&config, 20, &Operation::remove_worktree(&remove)).unwrap();
    drop(supervisor);

    let supervisor = Supervisor::start(config.clone(), SupervisorBoundary::default())
        .await
        .unwrap();
    assert!(!crate::worktree::expected_path(&config.home, workspace, agent_id).exists());
    assert!(matches!(
        mutation_journal::begin::<WorktreeReceipt>(&config, &add_operation).unwrap(),
        Begin::Failed(error) if error.code == "mutation_authority_superseded"
    ));
    credential(&supervisor).await;
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::AddWorktree(fenced(19, add)))
                .await
        )
        .code,
        "mutation_authority_retired"
    );
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::RemoveWorktree(fenced(20, remove)))
            .await,
        ResponseFrame::RemoveWorktree(_)
    ));
    cleanup(&config);
}

#[tokio::test]
async fn exact_completed_retry_uses_the_new_authority_fence() {
    let config = RuntimeConfig::for_test(root("higher-fence-replay"));
    let supervisor = provision(&config).await;
    let workspace = "replay";
    let base = prepare_workspace(&supervisor, workspace).await;
    let request = WorktreeRequest::new(
        format!("worktree:{workspace}:agent-replay:add:v1"),
        workspace,
        "agent-replay",
        base,
    );
    let first = match supervisor
        .dispatch(RequestFrame::AddWorktree(fenced(7, request.clone())))
        .await
    {
        ResponseFrame::AddWorktree(receipt) => receipt,
        other => panic!("worktree addition failed: {other:?}"),
    };
    let replay = match supervisor
        .dispatch(RequestFrame::AddWorktree(fenced(9, request)))
        .await
    {
        ResponseFrame::AddWorktree(receipt) => receipt,
        other => panic!("worktree replay failed: {other:?}"),
    };
    assert_eq!(first.authority_fence, 7);
    assert_eq!(replay.authority_fence, 9);
    assert_eq!(replay.receipt, first.receipt);
    cleanup(&config);
}

#[tokio::test(flavor = "current_thread")]
async fn create_ahead_of_cleanup_completes_first_and_cleanup_wins() {
    let config = RuntimeConfig::for_test(root("serialized-create-cleanup"));
    let supervisor = provision(&config).await;
    let workspace = "serialized";
    let base = prepare_workspace(&supervisor, workspace).await;
    let agent_id = "agent-serialized";
    let add = WorktreeRequest::new(
        format!("worktree:{workspace}:{agent_id}:add:v1"),
        workspace,
        agent_id,
        base,
    );
    let remove = WorktreeRef::new(
        format!("worktree:{workspace}:{agent_id}:remove:v1"),
        workspace,
        agent_id,
    );

    let gate = supervisor.operation_gate.lock().await;
    let create = tokio::spawn({
        let supervisor = Arc::clone(&supervisor);
        async move {
            supervisor
                .dispatch(RequestFrame::AddWorktree(fenced(30, add)))
                .await
        }
    });
    tokio::task::yield_now().await;
    let remove = tokio::spawn({
        let supervisor = Arc::clone(&supervisor);
        async move {
            supervisor
                .dispatch(RequestFrame::RemoveWorktree(fenced(31, remove)))
                .await
        }
    });
    tokio::task::yield_now().await;
    drop(gate);

    assert!(matches!(
        create.await.unwrap(),
        ResponseFrame::AddWorktree(_)
    ));
    assert!(matches!(
        remove.await.unwrap(),
        ResponseFrame::RemoveWorktree(_)
    ));
    assert!(!crate::worktree::expected_path(&config.home, workspace, agent_id).exists());
    cleanup(&config);
}
