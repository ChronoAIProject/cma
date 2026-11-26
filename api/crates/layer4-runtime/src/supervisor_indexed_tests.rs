//! Explicit test selection; production format selection is a separate boundary.
use super::*;
use crate::mutation_journal::Operation;
use layer3_adapter::wire::{AgentOpenReceipt, FencedRequest, WireFailure};
use layer3_adapter::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, CredentialFrame, HomeFile,
    RuntimeCredential, WorkspaceReceipt, WorkspaceRequest, WorkspaceSource, WorktreeReceipt,
    WorktreeRef, WorktreeRequest,
};
#[path = "supervisor_indexed_fixture.rs"]
mod fixture;
use crate::mutation_ledger::Ledger;
use crate::mutation_store::{Begin as StoredBegin, Store};
use fixture::*;
#[path = "supervisor_ledger_recovery_tests.rs"]
mod recovery_faults;

fn indexed_config() -> RuntimeConfig {
    RuntimeConfig::for_test(
        std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-indexed-supervisor-{}", uuid::Uuid::new_v4())),
    )
}
async fn indexed(config: &RuntimeConfig, pages: u32) -> (Arc<Supervisor>, Arc<Ledger>) {
    let mut supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    crate::fs_durability::create_dir_all(&config.state_root).unwrap();
    let directory = config.state_root.join("indexed-test");
    let create = !directory.exists();
    let ledger = Arc::new(
        Ledger::open(directory, config.clone(), create, pages)
            .await
            .unwrap(),
    );
    Arc::get_mut(&mut supervisor).unwrap().mutations =
        Store::new(config.clone(), Some(Arc::clone(&ledger)));
    if create {
        assert!(matches!(
            supervisor
                .dispatch(RequestFrame::Install(InstallRequest::new(
                    "install-indexed-test",
                    Vec::new()
                )))
                .await,
            ResponseFrame::Install(_)
        ));
    } else {
        supervisor.recover_journals().await.unwrap();
    }
    credential(&supervisor).await;
    (supervisor, ledger)
}
async fn close_store(supervisor: Arc<Supervisor>, ledger: Arc<Ledger>) {
    drop(supervisor);
    Arc::try_unwrap(ledger)
        .ok()
        .expect("all tickets completed")
        .close()
        .await
        .unwrap();
}

#[path = "supervisor_indexed_restart_teardown_tests.rs"]
mod restart_teardown;

#[tokio::test]
async fn indexed_same_runtime_restart_recovers_an_interrupted_close_for_the_exact_process() {
    use crate::mutation_ledger::ProcessFilter;

    let mut config = indexed_config();
    config.agent_process_restart_enabled = true;
    config.home_identity = Some("a".repeat(43));
    std::fs::create_dir_all(&config.home).unwrap();
    let (supervisor, ledger) = indexed(&config, 32_768).await;
    let workspace = prepare_workspace(&supervisor).await;
    let (_, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    let home = home_plan("agent-1", b"retained conversation fixture");
    assert!(matches!(
        supervisor.dispatch(materialize_home_request(home)).await,
        ResponseFrame::MaterializeAgentHome(_)
    ));
    let mut launch = launch(&config, &worktree, "while IFS= read -r line; do :; done");
    assert!(matches!(
        supervisor
            .dispatch(open_agent_request(launch.clone()))
            .await,
        ResponseFrame::OpenAgent(_)
    ));
    let old = AgentCloseRequest::new("agent:agent-1:close:v1", launch.agent_id.clone());
    let old_close = RequestFrame::CloseAgent(FencedRequest::new(2, old));
    let closed = supervisor.dispatch(old_close.clone()).await;
    assert!(matches!(closed, ResponseFrame::CloseAgent(_)));
    launch.operation_id = "agent:agent-1:open:2:v1".into();
    launch.request_sha256 = "c".repeat(64);
    let pinned = super::persistent_home_tests::restart_open(&supervisor, 3, &launch);
    let opened = supervisor.dispatch(pinned.clone()).await;
    assert!(matches!(opened, ResponseFrame::OpenAgent(_)), "{opened:?}");
    // An uncertain caller can replay the same request while its bridge remains live.
    assert_eq!(supervisor.dispatch(pinned.clone()).await, opened);
    assert_eq!(supervisor.dispatch(old_close.clone()).await, closed);
    assert!(ledger
        .active(ProcessFilter::Agent("agent-1".into()))
        .await
        .unwrap());
    close_store(supervisor, ledger).await;

    config.agent_process_restart_enabled = false;
    let (supervisor, ledger) = indexed(&config, 32_768).await;
    assert_eq!(
        failure(supervisor.dispatch(pinned).await).code,
        "agent_process_restart_runtime_changed"
    );
    assert!(supervisor.bridge("agent-1").await.is_none());
    assert_eq!(supervisor.dispatch(old_close).await, closed);
    assert!(ledger.active(ProcessFilter::Any).await.unwrap());
    let close = AgentCloseRequest::new("agent:agent-1:close:2:v1", launch.agent_id);
    let operation = Operation::close_agent(&close);
    ledger.admit(4, vec![operation.clone()]).await.unwrap();
    assert!(ledger.begin(operation.clone()).await.unwrap().new);
    close_store(supervisor, ledger).await;

    let (supervisor, ledger) = indexed(&config, 32_768).await;
    let response = supervisor
        .dispatch(RequestFrame::CloseAgent(FencedRequest::new(4, close)))
        .await;
    assert!(
        matches!(response, ResponseFrame::CloseAgent(_)),
        "{response:?}"
    );
    assert!(ledger.completed(operation).await.unwrap());
    assert!(!ledger.active(ProcessFilter::Any).await.unwrap());
    assert!(config.agent_home("agent-1").join("config.toml").is_file());
    close_store(supervisor, ledger).await;
    cleanup(&config);
}

#[tokio::test]
async fn indexed_supervisor_replays_exactly_and_checks_bridge_before_completed_open() {
    let config = indexed_config();
    let (supervisor, ledger) = indexed(&config, 32_768).await;
    let workspace = prepare_workspace(&supervisor).await;
    let (_, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    let plan = home_plan("agent-1", b"preserved home");
    let home = supervisor
        .dispatch(materialize_home_request(plan.clone()))
        .await;
    assert!(matches!(home, ResponseFrame::MaterializeAgentHome(_)));
    assert_eq!(
        supervisor.dispatch(materialize_home_request(plan)).await,
        home
    );
    let launch = launch(
        &config,
        &worktree,
        "while IFS= read -r line; do printf '%s\\n' \"$line\"; done",
    );
    let opened = supervisor
        .dispatch(open_agent_request(launch.clone()))
        .await;
    assert!(matches!(opened, ResponseFrame::OpenAgent(_)), "{opened:?}");
    assert_eq!(
        supervisor
            .dispatch(open_agent_request(launch.clone()))
            .await,
        opened
    );
    supervisor
        .bridge("agent-1")
        .await
        .unwrap()
        .terminate()
        .await
        .unwrap();
    assert_eq!(
        failure(supervisor.dispatch(open_agent_request(launch)).await).code,
        "agent_process_not_live"
    );
    let remove_home =
        AgentHomeRemoveRequest::new("agent:agent-1:home:remove:v1", AgentSlot::new("agent-1"));
    assert_eq!(
        failure(
            supervisor
                .dispatch(remove_home_request(remove_home.clone()))
                .await
        )
        .code,
        "agent_running"
    );
    let remove_worktree =
        WorktreeRef::new("worktree:project:agent-1:remove:v1", "project", "agent-1");
    assert_eq!(
        failure(
            supervisor
                .dispatch(remove_worktree_request(remove_worktree.clone()))
                .await
        )
        .code,
        "worktree_agent_open"
    );
    let close = AgentCloseRequest::new("agent:agent-1:close:v1", AgentSlot::new("agent-1"));
    let closed = supervisor
        .dispatch(close_agent_request(close.clone()))
        .await;
    assert!(matches!(closed, ResponseFrame::CloseAgent(_)), "{closed:?}");
    let removed = supervisor
        .dispatch(remove_home_request(remove_home.clone()))
        .await;
    assert!(
        matches!(removed, ResponseFrame::RemoveAgentHome(_)),
        "{removed:?}"
    );
    assert!(matches!(
        supervisor
            .dispatch(remove_worktree_request(remove_worktree))
            .await,
        ResponseFrame::RemoveWorktree(_)
    ));
    close_store(supervisor, ledger).await;
    let (supervisor, ledger) = indexed(&config, 32_768).await;
    assert_eq!(
        supervisor.dispatch(close_agent_request(close)).await,
        closed
    );
    assert_eq!(
        supervisor.dispatch(remove_home_request(remove_home)).await,
        removed
    );
    close_store(supervisor, ledger).await;
    cleanup(&config);
}

#[tokio::test]
async fn indexed_started_open_recreation_is_indeterminate_and_never_spawns() {
    let config = indexed_config();
    let (supervisor, ledger) = indexed(&config, 32_768).await;
    let workspace = prepare_workspace(&supervisor).await;
    let (_, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    assert!(matches!(
        supervisor
            .dispatch(materialize_home_request(home_plan("agent-1", b"preserved")))
            .await,
        ResponseFrame::MaterializeAgentHome(_)
    ));
    let launch = launch(
        &config,
        &worktree,
        "touch \"$HOME/spawned\"; while IFS= read -r line; do printf '%s\\n' \"$line\"; done",
    );
    let operation = Operation::open_agent(&launch);
    supervisor
        .mutations
        .admit(1, std::slice::from_ref(&operation))
        .await
        .unwrap();
    assert!(matches!(
        supervisor
            .mutations
            .begin::<AgentOpenReceipt>(&operation)
            .await
            .unwrap(),
        StoredBegin::New(_)
    ));
    close_store(supervisor, ledger).await;
    let (supervisor, ledger) = indexed(&config, 32_768).await;
    assert!(!config.agent_home("agent-1").join("spawned").exists());
    assert!(supervisor.bridge("agent-1").await.is_none());
    assert_eq!(
        failure(
            supervisor
                .dispatch(open_agent_request(launch.clone()))
                .await
        )
        .code,
        "agent_open_outcome_unknown"
    );
    let mut changed = launch;
    changed.args.push("changed".into());
    assert_eq!(
        failure(supervisor.dispatch(open_agent_request(changed)).await).code,
        "mutation_authority_conflict"
    );
    assert!(!config.agent_home("agent-1").join("spawned").exists());
    close_store(supervisor, ledger).await;
    cleanup(&config);
}

#[tokio::test]
async fn indexed_capacity_refuses_the_whole_preparation_pair_before_effects() {
    let config = indexed_config();
    let (supervisor, ledger) = indexed(&config, 16_384).await;
    let workspace = prepare_workspace(&supervisor).await;
    for i in 0..100 {
        let operation = Operation::close_agent(&AgentCloseRequest::new(
            format!("agent:fill-{i}:close:v1"),
            AgentSlot::new(format!("fill-{i}")),
        ));
        if let Err(error) = supervisor.mutations.admit(1, &[operation]).await {
            assert_eq!(error.code, "mutation_ledger_capacity");
            break;
        }
    }
    let before = ledger.snapshot().await.unwrap();
    let worktree = WorktreeRequest::new(
        "worktree:project:agent-1:add:v1",
        "project",
        "agent-1",
        workspace.resolved_commit,
    );
    let result = supervisor
        .dispatch(RequestFrame::PrepareAgentResources(FencedRequest::new(
            1,
            layer3_adapter::wire::AgentResourcesPlan {
                worktree,
                home: home_plan("agent-1", b"must not publish"),
            },
        )))
        .await;
    assert_eq!(failure(result).code, "mutation_ledger_capacity");
    assert_eq!(ledger.snapshot().await.unwrap(), before);
    assert!(!crate::worktree::expected_path(&config.home, "project", "agent-1").exists());
    assert!(!config.agent_home("agent-1").exists());
    close_store(supervisor, ledger).await;
    cleanup(&config);
}

#[tokio::test]
async fn indexed_cleanup_admission_supersedes_started_create_before_resource_recovery() {
    let config = indexed_config();
    let (supervisor, ledger) = indexed(&config, 32_768).await;
    let workspace = prepare_workspace(&supervisor).await;
    let request = WorktreeRequest::new(
        "worktree:project:agent-1:add:v1",
        "project",
        "agent-1",
        workspace.resolved_commit,
    );
    let operation = Operation::add_worktree(&request);
    supervisor
        .mutations
        .admit(1, std::slice::from_ref(&operation))
        .await
        .unwrap();
    assert!(matches!(
        supervisor
            .mutations
            .begin::<WorktreeReceipt>(&operation)
            .await
            .unwrap(),
        StoredBegin::New(_)
    ));
    let cleanup_request =
        WorktreeRef::new("worktree:project:agent-1:remove:v1", "project", "agent-1");
    let response = supervisor
        .dispatch(RequestFrame::RemoveWorktree(FencedRequest::new(
            2,
            cleanup_request,
        )))
        .await;
    assert!(
        matches!(response, ResponseFrame::RemoveWorktree(_)),
        "{response:?}"
    );
    assert!(!crate::worktree::expected_path(&config.home, "project", "agent-1").exists());
    assert!(
        matches!(supervisor.mutations.begin::<WorktreeReceipt>(&operation).await.unwrap(),StoredBegin::Failed(error) if error.code=="mutation_authority_superseded")
    );
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::AddWorktree(FencedRequest::new(3, request)))
                .await
        )
        .code,
        "mutation_authority_retired"
    );
    close_store(supervisor, ledger).await;
    cleanup(&config);
}
