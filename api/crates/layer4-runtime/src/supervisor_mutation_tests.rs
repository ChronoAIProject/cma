use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::wire::{
    AgentCloseReceipt, AgentHomeReceipt, AgentHomeRemoveReceipt, AgentOpenReceipt, FencedReceipt,
    FencedRequest, RequestFrame, ResponseFrame, WireFailure,
};
use layer3_adapter::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch, AgentSlot,
    CredentialFrame, HomeFile, InstallRequest, RuntimeCredential, WorkspaceReceipt,
    WorkspaceRemoveRequest, WorkspaceRequest, WorkspaceSetupRequest, WorkspaceSource,
    WorktreeReceipt, WorktreeRef, WorktreeRemoveReceipt, WorktreeRequest,
};
use sha2::{Digest, Sha256};

use super::*;
use crate::mutation_journal::{self, Begin, Operation};

#[path = "supervisor_agent_resources_tests.rs"]
mod agent_resources_tests;
#[path = "supervisor_warmup_tests.rs"]
mod warmup_tests;

#[path = "supervisor_worktree_successor_tests.rs"]
mod worktree_successor_tests;

#[path = "supervisor_workspace_reconciliation_tests.rs"]
mod workspace_reconciliation_tests;
#[path = "supervisor_workspace_removal_tests.rs"]
mod workspace_removal_tests;

fn add_worktree_request(request: WorktreeRequest) -> RequestFrame {
    RequestFrame::AddWorktree(FencedRequest::new(1, request))
}

fn remove_worktree_request(request: WorktreeRef) -> RequestFrame {
    RequestFrame::RemoveWorktree(FencedRequest::new(1, request))
}

fn materialize_home_request(request: AgentHomePlan) -> RequestFrame {
    RequestFrame::MaterializeAgentHome(FencedRequest::new(1, request))
}

fn remove_home_request(request: AgentHomeRemoveRequest) -> RequestFrame {
    RequestFrame::RemoveAgentHome(FencedRequest::new(1, request))
}

fn open_agent_request(request: AgentLaunch) -> RequestFrame {
    RequestFrame::OpenAgent(FencedRequest::new(1, request))
}

fn close_agent_request(request: AgentCloseRequest) -> RequestFrame {
    RequestFrame::CloseAgent(FencedRequest::new(1, request))
}

fn fenced_receipt<T>(receipt: T) -> FencedReceipt<T> {
    FencedReceipt::new(1, receipt)
}

fn root(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "cma-supervisor-mutations-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn cleanup(config: &RuntimeConfig) {
    let _ = std::fs::remove_dir_all(&config.home);
    let _ = std::fs::remove_dir_all(&config.checkpoint_root);
    let _ = std::fs::remove_dir_all(&config.state_root);
}

fn failure(response: ResponseFrame) -> WireFailure {
    match response {
        ResponseFrame::Failure(failure) => failure,
        other => panic!("expected failure response, got {other:?}"),
    }
}

async fn wait_for_file(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while std::fs::metadata(path).map_or(true, |metadata| metadata.len() == 0) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent produced its fixture file");
}

async fn provision(config: &RuntimeConfig) -> Arc<Supervisor> {
    let supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    let installed = supervisor
        .dispatch(RequestFrame::Install(InstallRequest::new(
            "install-mutation-tests",
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
            operation_id: "credential-mutation-tests".into(),
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

async fn prepare_workspace(supervisor: &Supervisor) -> WorkspaceReceipt {
    match supervisor
        .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
            "workspace:wks_mutations:prepare:v1",
            "wks_mutations",
            "project",
            WorkspaceSource::Init,
        )))
        .await
    {
        ResponseFrame::PrepareWorkspace(receipt) => receipt,
        other => panic!("workspace preparation failed: {other:?}"),
    }
}

#[tokio::test]
async fn workspace_removal_before_first_prepare_survives_restart_and_name_reuse() {
    let config = RuntimeConfig::for_test(root("workspace-remove-before-prepare"));
    let supervisor = provision(&config).await;
    let remove = WorkspaceRemoveRequest::new("workspace:wks_old:remove:v1", "wks_old", "project");
    let removed = supervisor
        .dispatch(RequestFrame::RemoveWorkspace(remove.clone()))
        .await;
    assert!(
        matches!(removed, ResponseFrame::RemoveWorkspace(_)),
        "{removed:?}"
    );
    assert!(!config.home.join("project").exists());
    drop(supervisor);

    let supervisor = recreate(&config).await;
    let delayed = WorkspaceRequest::new(
        "workspace:wks_old:prepare:v1",
        "wks_old",
        "project",
        WorkspaceSource::Init,
    );
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::PrepareWorkspace(delayed.clone()))
                .await
        )
        .code,
        "workspace_removed"
    );
    assert!(!config.home.join("project").exists());
    let successor = WorkspaceRequest::new(
        "workspace:wks_successor:prepare:v1",
        "wks_successor",
        "project",
        WorkspaceSource::Init,
    );
    let prepared = supervisor
        .dispatch(RequestFrame::PrepareWorkspace(successor))
        .await;
    assert!(
        matches!(prepared, ResponseFrame::PrepareWorkspace(_)),
        "{prepared:?}"
    );
    let setup = WorkspaceSetupRequest::new(
        "workspace:wks_old:setup:v1",
        "wks_old",
        "project",
        vec!["touch stale-setup".into()],
    );
    for request in [
        RequestFrame::PrepareWorkspace(delayed.clone()),
        RequestFrame::WorkspaceSetup(setup),
    ] {
        let error = failure(supervisor.dispatch(request).await);
        assert_eq!(error.code, "workspace_removed");
        assert!(!error.retryable);
    }
    assert_eq!(
        supervisor
            .dispatch(RequestFrame::RemoveWorkspace(remove))
            .await,
        removed
    );
    assert!(!config.home.join("project/stale-setup").exists());
    crate::workspace_source::require_workspace_identity(
        &config.home.join("project"),
        "wks_successor",
    )
    .unwrap();
    drop(supervisor);

    let supervisor = recreate(&config).await;
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::PrepareWorkspace(delayed))
                .await
        )
        .code,
        "workspace_removed"
    );
    crate::workspace_source::require_workspace_identity(
        &config.home.join("project"),
        "wks_successor",
    )
    .unwrap();
    assert!(!config.home.join(".cma/preparing/wks_old").exists());
    cleanup(&config);
}

#[tokio::test]
async fn workspace_removal_overrides_completed_prepare_and_setup_replays() {
    let config = RuntimeConfig::for_test(root("workspace-remove-completed"));
    let supervisor = provision(&config).await;
    let prepare = WorkspaceRequest::new(
        "workspace:wks_mutations:prepare:v1",
        "wks_mutations",
        "project",
        WorkspaceSource::Init,
    );
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::PrepareWorkspace(prepare.clone()))
            .await,
        ResponseFrame::PrepareWorkspace(_)
    ));
    let setup = WorkspaceSetupRequest::new(
        "workspace:wks_mutations:setup:v1",
        "wks_mutations",
        "project",
        vec!["touch first-setup".into()],
    );
    let response = supervisor
        .dispatch(RequestFrame::WorkspaceSetup(setup.clone()))
        .await;
    assert!(
        matches!(response, ResponseFrame::WorkspaceSetup(_)),
        "{response:?}"
    );
    assert!(config.home.join("project/first-setup").exists());
    let response = supervisor
        .dispatch(RequestFrame::RemoveWorkspace(WorkspaceRemoveRequest::new(
            "workspace:wks_mutations:remove:v1",
            "wks_mutations",
            "project",
        )))
        .await;
    assert!(
        matches!(response, ResponseFrame::RemoveWorkspace(_)),
        "{response:?}"
    );
    drop(supervisor);

    let supervisor = recreate(&config).await;
    for request in [
        RequestFrame::PrepareWorkspace(prepare),
        RequestFrame::WorkspaceSetup(setup),
    ] {
        assert_eq!(
            failure(supervisor.dispatch(request).await).code,
            "workspace_removed"
        );
    }
    assert!(!config.home.join("project").exists());
    cleanup(&config);
}

async fn add_worktree(supervisor: &Supervisor, base: String) -> (WorktreeRequest, WorktreeReceipt) {
    let request = WorktreeRequest::new(
        "worktree:project:agent-1:add:v1",
        "project",
        "agent-1",
        base,
    );
    let receipt = match supervisor
        .dispatch(add_worktree_request(request.clone()))
        .await
    {
        ResponseFrame::AddWorktree(receipt) => receipt.receipt,
        other => panic!("worktree addition failed: {other:?}"),
    };
    (request, receipt)
}

fn home_plan(agent_id: &str, contents: &[u8]) -> AgentHomePlan {
    AgentHomePlan {
        operation_id: format!("agent:{agent_id}:home:create:v1"),
        request_sha256: "a".repeat(64),
        agent_id: AgentSlot::new(agent_id),
        files: vec![HomeFile {
            path: "config.toml".into(),
            bytes: contents.to_vec(),
        }],
        symlinks: Vec::new(),
        skills: Vec::new(),
    }
}

fn launch(config: &RuntimeConfig, worktree: &WorktreeReceipt, script: &str) -> AgentLaunch {
    AgentLaunch {
        operation_id: "agent:agent-1:open:v1".into(),
        request_sha256: "b".repeat(64),
        agent_id: AgentSlot::new("agent-1"),
        cwd: worktree.path.clone(),
        home: config.agent_home("agent-1").to_string_lossy().into_owned(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        env: Default::default(),
    }
}

#[tokio::test]
async fn restored_open_identity_refusal_does_not_consume_the_reconnect_fence_or_replace_process() {
    let config = RuntimeConfig::for_test(root("restored-open-reconnect"));
    let supervisor = provision(&config).await;
    let workspace = prepare_workspace(&supervisor).await;
    let (_, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    assert!(matches!(
        supervisor
            .dispatch(materialize_home_request(home_plan("agent-1", b"profile")))
            .await,
        ResponseFrame::MaterializeAgentHome(_)
    ));
    let mut restored_launch = launch(&config, &worktree, "while IFS= read -r line; do :; done");
    let options = layer3_adapter::RuntimeOptions {
        working_directory: restored_launch.cwd.clone(),
        home_directory: restored_launch.home.clone(),
        slot: restored_launch.agent_id.clone(),
        ..Default::default()
    };
    let original = layer3_adapter::AgentOpenRequest::new(
        restored_launch.operation_id.clone(),
        options.clone(),
        None,
    );
    let restored = layer3_adapter::AgentOpenRequest::new(
        restored_launch.operation_id.clone(),
        options,
        Some("recorded-conversation".into()),
    );
    assert_ne!(original.request_sha256, restored.request_sha256);
    restored_launch.request_sha256 = restored.request_sha256.clone();
    let opened = supervisor
        .dispatch(RequestFrame::OpenAgent(FencedRequest::new(
            2,
            restored_launch.clone(),
        )))
        .await;
    assert!(matches!(opened, ResponseFrame::OpenAgent(_)), "{opened:?}");
    let bridge = supervisor
        .agents
        .read()
        .await
        .get(&restored_launch.agent_id)
        .unwrap()
        .bridge
        .clone();
    let mut original_launch = restored_launch.clone();
    original_launch.request_sha256 = original.request_sha256;
    let refused = failure(
        supervisor
            .dispatch(RequestFrame::OpenAgent(FencedRequest::new(
                3,
                original_launch,
            )))
            .await,
    );
    assert_eq!(refused.code, "mutation_authority_conflict");
    assert!(!refused.retryable);
    let replayed = supervisor
        .dispatch(RequestFrame::OpenAgent(FencedRequest::new(
            3,
            restored_launch.clone(),
        )))
        .await;
    let ResponseFrame::OpenAgent(receipt) = replayed else {
        panic!("{replayed:?}")
    };
    assert_eq!(receipt.authority_fence, 3);
    assert_eq!(receipt.receipt.request_sha256, restored.request_sha256);
    assert!(Arc::ptr_eq(
        &bridge,
        &supervisor
            .agents
            .read()
            .await
            .get(&restored_launch.agent_id)
            .unwrap()
            .bridge
    ));
    assert!(matches!(
        mutation_journal::begin::<AgentOpenReceipt>(
            &config,
            &Operation::open_agent(&restored_launch)
        )
        .unwrap(),
        Begin::Completed(_)
    ));
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::CloseAgent(FencedRequest::new(
                4,
                AgentCloseRequest::new("agent:agent-1:close:v1", restored_launch.agent_id)
            )))
            .await,
        ResponseFrame::CloseAgent(_)
    ));
    drop(bridge);
    drop(supervisor);
    cleanup(&config);
}

#[tokio::test]
async fn absent_cleanup_intents_complete_with_exact_durable_receipts() {
    let config = RuntimeConfig::for_test(root("absent-cleanup"));
    let supervisor = provision(&config).await;
    let agent_id = AgentSlot::new("agent-absent");

    let invalid_close = AgentCloseRequest::new("agent:another-agent:close:v1", agent_id.clone());
    assert_eq!(
        failure(
            supervisor
                .dispatch(close_agent_request(invalid_close))
                .await
        )
        .code,
        "agent_operation_invalid"
    );
    let invalid_home =
        AgentHomeRemoveRequest::new("agent:another-agent:home:remove:v1", agent_id.clone());
    assert_eq!(
        failure(supervisor.dispatch(remove_home_request(invalid_home)).await).code,
        "agent_operation_invalid"
    );
    let invalid_worktree = WorktreeRef::new(
        "worktree:workspace-absent:another-agent:remove:v1",
        "workspace-absent",
        "agent-absent",
    );
    assert_eq!(
        failure(
            supervisor
                .dispatch(remove_worktree_request(invalid_worktree))
                .await
        )
        .code,
        "worktree_operation_invalid"
    );

    let close = AgentCloseRequest::new("agent:agent-absent:close:v1", agent_id.clone());
    let expected_close = ResponseFrame::CloseAgent(fenced_receipt(AgentCloseReceipt {
        operation_id: close.operation_id.clone(),
        request_sha256: close.request_sha256.clone(),
        agent_id: agent_id.clone(),
    }));
    assert_eq!(
        supervisor
            .dispatch(close_agent_request(close.clone()))
            .await,
        expected_close
    );

    let remove_home =
        AgentHomeRemoveRequest::new("agent:agent-absent:home:remove:v1", agent_id.clone());
    let expected_home = ResponseFrame::RemoveAgentHome(fenced_receipt(AgentHomeRemoveReceipt {
        operation_id: remove_home.operation_id.clone(),
        request_sha256: remove_home.request_sha256.clone(),
        agent_id,
    }));
    assert_eq!(
        supervisor
            .dispatch(remove_home_request(remove_home.clone()))
            .await,
        expected_home
    );

    let remove_worktree = WorktreeRef::new(
        "worktree:workspace-absent:agent-absent:remove:v1",
        "workspace-absent",
        "agent-absent",
    );
    let expected_worktree = ResponseFrame::RemoveWorktree(fenced_receipt(WorktreeRemoveReceipt {
        operation_id: remove_worktree.operation_id.clone(),
        request_sha256: remove_worktree.request_sha256.clone(),
        workspace: remove_worktree.workspace.clone(),
        name: remove_worktree.name.clone(),
    }));
    assert_eq!(
        supervisor
            .dispatch(remove_worktree_request(remove_worktree.clone()))
            .await,
        expected_worktree
    );
    drop(supervisor);

    let supervisor = recreate(&config).await;
    assert_eq!(
        supervisor.dispatch(close_agent_request(close)).await,
        expected_close
    );
    assert_eq!(
        supervisor.dispatch(remove_home_request(remove_home)).await,
        expected_home
    );
    assert_eq!(
        supervisor
            .dispatch(remove_worktree_request(remove_worktree))
            .await,
        expected_worktree
    );
    cleanup(&config);
}

#[tokio::test]
async fn fresh_worktree_removal_completes_partial_cleanup() {
    let config = RuntimeConfig::for_test(root("fresh-partial-worktree-removal"));
    let supervisor = provision(&config).await;
    let workspace = prepare_workspace(&supervisor).await;
    let (_, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    let repository = config.home.join("project");
    let git = crate::git::GitRunner::new(true);
    git.checked(
        &repository,
        ["worktree", "remove", "--force", worktree.path.as_str()],
        "partially remove fixture worktree",
    )
    .await
    .unwrap();

    let request = WorktreeRef::new("worktree:project:agent-1:remove:v1", "project", "agent-1");
    let expected = ResponseFrame::RemoveWorktree(fenced_receipt(WorktreeRemoveReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace: request.workspace.clone(),
        name: request.name.clone(),
    }));
    assert_eq!(
        supervisor
            .dispatch(remove_worktree_request(request.clone()))
            .await,
        expected
    );
    let branches = git
        .checked(
            &repository,
            [
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads/cma/agent-1",
            ],
            "inspect removed worktree branch",
        )
        .await
        .unwrap();
    assert!(branches.stdout.trim().is_empty());
    assert_eq!(
        supervisor.dispatch(remove_worktree_request(request)).await,
        expected
    );
    cleanup(&config);
}

#[tokio::test]
async fn all_mutation_classes_replay_exactly_and_reject_changed_requests() {
    let config = RuntimeConfig::for_test(root("all-replays"));
    let supervisor = provision(&config).await;
    let workspace = prepare_workspace(&supervisor).await;
    let (add_request, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    let add_response = ResponseFrame::AddWorktree(fenced_receipt(worktree.clone()));
    assert_eq!(
        supervisor
            .dispatch(add_worktree_request(add_request.clone()))
            .await,
        add_response
    );
    let mut changed_add = add_request;
    changed_add.base = "HEAD".into();
    changed_add.request_sha256 = changed_add.computed_sha256();
    assert_eq!(
        failure(supervisor.dispatch(add_worktree_request(changed_add)).await).code,
        "mutation_authority_conflict"
    );

    let plan = home_plan("agent-1", b"original");
    let home_response = supervisor
        .dispatch(materialize_home_request(plan.clone()))
        .await;
    assert!(
        matches!(home_response, ResponseFrame::MaterializeAgentHome(_)),
        "{home_response:?}"
    );
    assert_eq!(
        supervisor
            .dispatch(materialize_home_request(plan.clone()))
            .await,
        home_response
    );
    let mut changed_plan = plan;
    changed_plan.files[0].bytes = b"changed".to_vec();
    assert_eq!(
        failure(
            supervisor
                .dispatch(materialize_home_request(changed_plan))
                .await
        )
        .code,
        "mutation_authority_conflict"
    );

    let launch = launch(
        &config,
        &worktree,
        "while IFS= read -r line; do printf '%s\\n' \"$line\"; done",
    );
    let open_response = supervisor
        .dispatch(open_agent_request(launch.clone()))
        .await;
    assert!(
        matches!(open_response, ResponseFrame::OpenAgent(_)),
        "{open_response:?}"
    );
    assert_eq!(
        supervisor
            .dispatch(open_agent_request(launch.clone()))
            .await,
        open_response
    );
    let mut changed_launch = launch;
    changed_launch.args.push("changed".into());
    assert_eq!(
        failure(
            supervisor
                .dispatch(open_agent_request(changed_launch))
                .await
        )
        .code,
        "mutation_authority_conflict"
    );

    let close = AgentCloseRequest::new("agent:agent-1:close:v1", AgentSlot::new("agent-1"));
    let close_response = supervisor
        .dispatch(close_agent_request(close.clone()))
        .await;
    assert!(
        matches!(close_response, ResponseFrame::CloseAgent(_)),
        "{close_response:?}"
    );
    assert!(config.agent_home("agent-1").is_dir());
    assert_eq!(
        supervisor
            .dispatch(close_agent_request(close.clone()))
            .await,
        close_response
    );
    let changed_close = AgentCloseRequest::new(close.operation_id, AgentSlot::new("agent-2"));
    assert_eq!(
        failure(
            supervisor
                .dispatch(close_agent_request(changed_close))
                .await
        )
        .code,
        "agent_operation_invalid"
    );

    let remove_home =
        AgentHomeRemoveRequest::new("agent:agent-1:home:remove:v1", AgentSlot::new("agent-1"));
    let remove_home_response = supervisor
        .dispatch(remove_home_request(remove_home.clone()))
        .await;
    assert!(
        matches!(remove_home_response, ResponseFrame::RemoveAgentHome(_)),
        "{remove_home_response:?}"
    );
    assert_eq!(
        supervisor
            .dispatch(remove_home_request(remove_home.clone()))
            .await,
        remove_home_response
    );
    let changed_remove_home =
        AgentHomeRemoveRequest::new(remove_home.operation_id, AgentSlot::new("agent-2"));
    assert_eq!(
        failure(
            supervisor
                .dispatch(remove_home_request(changed_remove_home))
                .await
        )
        .code,
        "agent_operation_invalid"
    );

    let remove_worktree =
        WorktreeRef::new("worktree:project:agent-1:remove:v1", "project", "agent-1");
    let remove_response = supervisor
        .dispatch(remove_worktree_request(remove_worktree.clone()))
        .await;
    assert!(
        matches!(remove_response, ResponseFrame::RemoveWorktree(_)),
        "{remove_response:?}"
    );
    assert_eq!(
        supervisor
            .dispatch(remove_worktree_request(remove_worktree.clone()))
            .await,
        remove_response
    );
    let changed_remove_worktree =
        WorktreeRef::new(remove_worktree.operation_id, "project", "agent-2");
    assert_eq!(
        failure(
            supervisor
                .dispatch(remove_worktree_request(changed_remove_worktree))
                .await
        )
        .code,
        "worktree_operation_invalid"
    );
    cleanup(&config);
}

#[tokio::test]
async fn completed_filesystem_mutations_replay_after_supervisor_recreation() {
    let config = RuntimeConfig::for_test(root("filesystem-recreation"));
    let supervisor = provision(&config).await;
    let workspace = prepare_workspace(&supervisor).await;
    let (add_request, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    drop(supervisor);

    let supervisor = recreate(&config).await;
    assert_eq!(
        supervisor.dispatch(add_worktree_request(add_request)).await,
        ResponseFrame::AddWorktree(fenced_receipt(worktree.clone()))
    );
    let plan = home_plan("agent-1", b"durable");
    let home_response = supervisor
        .dispatch(materialize_home_request(plan.clone()))
        .await;
    drop(supervisor);

    let supervisor = recreate(&config).await;
    assert_eq!(
        supervisor.dispatch(materialize_home_request(plan)).await,
        home_response
    );
    let remove_home =
        AgentHomeRemoveRequest::new("agent:agent-1:home:remove:v1", AgentSlot::new("agent-1"));
    let remove_home_response = supervisor
        .dispatch(remove_home_request(remove_home.clone()))
        .await;
    drop(supervisor);

    let supervisor = recreate(&config).await;
    assert_eq!(
        supervisor.dispatch(remove_home_request(remove_home)).await,
        remove_home_response
    );
    let remove_worktree =
        WorktreeRef::new("worktree:project:agent-1:remove:v1", "project", "agent-1");
    let remove_worktree_response = supervisor
        .dispatch(remove_worktree_request(remove_worktree.clone()))
        .await;
    drop(supervisor);

    let supervisor = recreate(&config).await;
    assert_eq!(
        supervisor
            .dispatch(remove_worktree_request(remove_worktree))
            .await,
        remove_worktree_response
    );
    cleanup(&config);
}

#[tokio::test]
async fn completed_open_requires_its_live_bridge_and_durable_lineage_blocks_cleanup() {
    let config = RuntimeConfig::for_test(root("open-recreation"));
    let original = provision(&config).await;
    let workspace = prepare_workspace(&original).await;
    let (_, worktree) = add_worktree(&original, workspace.resolved_commit).await;
    let plan = home_plan("agent-1", b"durable");
    assert!(matches!(
        original.dispatch(materialize_home_request(plan)).await,
        ResponseFrame::MaterializeAgentHome(_)
    ));
    let launch = launch(
        &config,
        &worktree,
        "while IFS= read -r line; do printf '%s\\n' \"$line\"; done",
    );
    let opened = original.dispatch(open_agent_request(launch.clone())).await;
    assert!(matches!(opened, ResponseFrame::OpenAgent(_)), "{opened:?}");
    assert_eq!(
        original.dispatch(open_agent_request(launch.clone())).await,
        opened
    );

    let recreated = recreate(&config).await;
    assert_eq!(
        failure(recreated.dispatch(open_agent_request(launch.clone())).await).code,
        "agent_process_not_live"
    );
    let blocked_home = failure(
        recreated
            .dispatch(remove_home_request(AgentHomeRemoveRequest::new(
                "agent:agent-1:home:remove:v1",
                AgentSlot::new("agent-1"),
            )))
            .await,
    );
    assert_eq!(blocked_home.code, "agent_running");
    let blocked_worktree = failure(
        recreated
            .dispatch(remove_worktree_request(WorktreeRef::new(
                "worktree:project:agent-1:remove:v1",
                "project",
                "agent-1",
            )))
            .await,
    );
    assert_eq!(blocked_worktree.code, "worktree_agent_open");

    let close = AgentCloseRequest::new("agent:agent-1:close:v1", AgentSlot::new("agent-1"));
    let closed = original.dispatch(close_agent_request(close.clone())).await;
    assert!(matches!(closed, ResponseFrame::CloseAgent(_)), "{closed:?}");
    assert!(config.agent_home("agent-1").is_dir());
    drop(recreated);

    let after_close = recreate(&config).await;
    assert_eq!(
        after_close.dispatch(close_agent_request(close)).await,
        closed
    );
    assert!(matches!(
        after_close
            .dispatch(remove_home_request(AgentHomeRemoveRequest::new(
                "agent:agent-1:home:remove:v1",
                AgentSlot::new("agent-1"),
            ),))
            .await,
        ResponseFrame::RemoveAgentHome(_)
    ));
    assert!(matches!(
        after_close
            .dispatch(remove_worktree_request(WorktreeRef::new(
                "worktree:project:agent-1:remove:v1",
                "project",
                "agent-1",
            )))
            .await,
        ResponseFrame::RemoveWorktree(_)
    ));
    cleanup(&config);
}

#[tokio::test]
async fn started_filesystem_mutations_converge_after_recreation() {
    let config = RuntimeConfig::for_test(root("started-filesystem"));
    let supervisor = provision(&config).await;
    let workspace = prepare_workspace(&supervisor).await;
    let add_request = WorktreeRequest::new(
        "worktree:project:agent-1:add:v1",
        "project",
        "agent-1",
        workspace.resolved_commit,
    );
    start_operation::<WorktreeReceipt>(&config, &Operation::add_worktree(&add_request));
    let worktree = crate::worktree::add(&config.home, true, &add_request)
        .await
        .unwrap();
    drop(supervisor);

    let supervisor = recreate(&config).await;
    assert_eq!(
        supervisor.dispatch(add_worktree_request(add_request)).await,
        ResponseFrame::AddWorktree(fenced_receipt(worktree.clone()))
    );

    let published_plan = home_plan("agent-1", b"published-before-terminal-record");
    start_operation::<AgentHomeReceipt>(&config, &Operation::create_agent_home(&published_plan));
    let published =
        crate::agent_home::materialize(&config, &reqwest::Client::new(), &published_plan)
            .await
            .unwrap();
    let staged_plan = home_plan("agent-2", b"retry-after-staging-cleanup");
    start_operation::<AgentHomeReceipt>(&config, &Operation::create_agent_home(&staged_plan));
    let staging = staging_path(&config, "agent-2", &staged_plan.operation_id);
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("partial"), b"incomplete").unwrap();
    drop(supervisor);

    let supervisor = recreate(&config).await;
    let replayed_home = supervisor
        .dispatch(materialize_home_request(published_plan.clone()))
        .await;
    assert!(matches!(
        replayed_home,
        ResponseFrame::MaterializeAgentHome(ref receipt)
            if receipt.receipt.path == published.to_string_lossy()
    ));
    assert!(!staging.exists());
    let retried_home = supervisor
        .dispatch(materialize_home_request(staged_plan))
        .await;
    assert!(
        matches!(retried_home, ResponseFrame::MaterializeAgentHome(_)),
        "{retried_home:?}"
    );

    let remove_home =
        AgentHomeRemoveRequest::new("agent:agent-1:home:remove:v1", AgentSlot::new("agent-1"));
    start_operation::<layer3_adapter::wire::AgentHomeRemoveReceipt>(
        &config,
        &Operation::remove_agent_home(&remove_home),
    );
    assert!(crate::agent_home::remove(&config, "agent-1").unwrap());

    let remove_worktree =
        WorktreeRef::new("worktree:project:agent-1:remove:v1", "project", "agent-1");
    start_operation::<layer3_adapter::WorktreeRemoveReceipt>(
        &config,
        &Operation::remove_worktree(&remove_worktree),
    );
    crate::git::GitRunner::new(true)
        .checked(
            &config.home.join("project"),
            ["worktree", "remove", "--force", worktree.path.as_str()],
            "partially remove fixture worktree",
        )
        .await
        .unwrap();
    drop(supervisor);

    let supervisor = recreate(&config).await;
    assert!(matches!(
        supervisor.dispatch(remove_home_request(remove_home)).await,
        ResponseFrame::RemoveAgentHome(_)
    ));
    assert!(matches!(
        supervisor
            .dispatch(remove_worktree_request(remove_worktree))
            .await,
        ResponseFrame::RemoveWorktree(_)
    ));
    cleanup(&config);
}

#[tokio::test]
async fn started_open_becomes_indeterminate_after_recreation_without_spawning() {
    let config = RuntimeConfig::for_test(root("started-open"));
    let supervisor = provision(&config).await;
    let workspace = prepare_workspace(&supervisor).await;
    let (_, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    let plan = home_plan("agent-1", b"durable");
    assert!(matches!(
        supervisor.dispatch(materialize_home_request(plan)).await,
        ResponseFrame::MaterializeAgentHome(_)
    ));
    let marker = config.agent_home("agent-1").join("spawned");
    let launch = launch(
        &config,
        &worktree,
        "touch \"$HOME/spawned\"; while IFS= read -r line; do printf '%s\\n' \"$line\"; done",
    );
    start_operation::<AgentOpenReceipt>(&config, &Operation::open_agent(&launch));
    drop(supervisor);

    let recreated = recreate(&config).await;
    assert!(!marker.exists());
    assert!(recreated.bridge("agent-1").await.is_none());
    let replay = failure(recreated.dispatch(open_agent_request(launch.clone())).await);
    assert_eq!(replay.code, "agent_open_outcome_unknown");
    assert!(!replay.retryable);
    assert!(!marker.exists());
    let blocked_home = failure(
        recreated
            .dispatch(remove_home_request(AgentHomeRemoveRequest::new(
                "agent:agent-1:home:remove:v1",
                AgentSlot::new("agent-1"),
            )))
            .await,
    );
    assert_eq!(blocked_home.code, "agent_running");
    let mut changed_launch = launch;
    changed_launch.args.push("changed".into());
    assert_eq!(
        failure(recreated.dispatch(open_agent_request(changed_launch)).await).code,
        "mutation_authority_conflict"
    );
    cleanup(&config);
}

#[tokio::test]
async fn invalid_launch_environment_is_refused_before_journal_admission() {
    let config = RuntimeConfig::for_test(root("invalid-launch-environment"));
    let supervisor = provision(&config).await;
    let workspace = prepare_workspace(&supervisor).await;
    let (_, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    let plan = home_plan("agent-1", b"durable");
    assert!(matches!(
        supervisor.dispatch(materialize_home_request(plan)).await,
        ResponseFrame::MaterializeAgentHome(_)
    ));
    let mut launch = launch(&config, &worktree, "exit 0");
    launch.env.insert("HOME".into(), "/tmp/override".into());
    let operation = Operation::open_agent(&launch);

    let first = failure(
        supervisor
            .dispatch(open_agent_request(launch.clone()))
            .await,
    );
    assert_eq!(first.code, "agent_environment_conflict");
    assert!(!mutation_journal::admitted(&config, &operation).unwrap());
    let replay = failure(supervisor.dispatch(open_agent_request(launch)).await);
    assert_eq!(replay.code, "agent_environment_conflict");
    cleanup(&config);
}

#[tokio::test]
async fn started_process_mutations_reconcile_without_repeating_side_effects() {
    let config = RuntimeConfig::for_test(root("started-process-boundaries"));
    let supervisor = provision(&config).await;
    let workspace = prepare_workspace(&supervisor).await;
    let (_, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    let plan = home_plan("agent-1", b"durable");
    assert!(matches!(
        supervisor.dispatch(materialize_home_request(plan)).await,
        ResponseFrame::MaterializeAgentHome(_)
    ));

    let starts = config.agent_home("agent-1").join("starts");
    let launch = launch(
        &config,
        &worktree,
        "printf 'started\\n' >> \"$HOME/starts\"; while IFS= read -r line; do printf '%s\\n' \"$line\"; done",
    );
    start_operation::<AgentOpenReceipt>(&config, &Operation::open_agent(&launch));
    let mut environment = config.process_env.clone();
    environment.insert("HOME".into(), launch.home.clone());
    let bridge = Arc::new(crate::bridge::Bridge::start(&launch, &environment).unwrap());
    supervisor.agents.write().await.insert(
        launch.agent_id.clone(),
        AgentProcess {
            launch: launch.clone(),
            bridge: Arc::clone(&bridge),
        },
    );
    wait_for_file(&starts).await;

    let opened = supervisor.open_agent(1, launch).await.unwrap();
    assert!(matches!(opened, ResponseFrame::OpenAgent(_)));
    assert_eq!(std::fs::read_to_string(&starts).unwrap(), "started\n");
    let retained = supervisor.bridge("agent-1").await.unwrap();
    assert!(Arc::ptr_eq(&bridge, &retained));

    let close = AgentCloseRequest::new("agent:agent-1:close:v1", AgentSlot::new("agent-1"));
    start_operation::<AgentCloseReceipt>(&config, &Operation::close_agent(&close));
    bridge.terminate().await.unwrap();
    supervisor.agents.write().await.remove(&close.agent_id);
    drop(retained);
    drop(bridge);
    drop(supervisor);

    let supervisor = recreate(&config).await;
    let expected_receipt = AgentCloseReceipt {
        operation_id: close.operation_id.clone(),
        request_sha256: close.request_sha256.clone(),
        agent_id: close.agent_id.clone(),
    };
    let expected = ResponseFrame::CloseAgent(fenced_receipt(expected_receipt.clone()));
    assert_eq!(
        supervisor
            .dispatch(close_agent_request(close.clone()))
            .await,
        expected
    );
    assert!(matches!(
        mutation_journal::begin::<AgentCloseReceipt>(&config, &Operation::close_agent(&close))
            .unwrap(),
        Begin::Completed(found) if found == expected_receipt
    ));
    assert!(config.agent_home("agent-1").is_dir());
    cleanup(&config);
}

#[tokio::test]
async fn corrupt_mutation_journal_refuses_startup() {
    let config = RuntimeConfig::for_test(root("corrupt-startup"));
    corrupt_mutation_journal(&config);
    let result = Supervisor::start(config.clone(), SupervisorBoundary::default()).await;
    let error = match result {
        Ok(_) => panic!("startup accepted a corrupt mutation journal"),
        Err(error) => error,
    };
    assert_eq!(error.code, "mutation_journal_corrupt");
    cleanup(&config);
}

#[tokio::test]
async fn post_start_mutation_journal_corruption_fences_mutations() {
    let config = RuntimeConfig::for_test(root("corrupt-after-start"));
    let supervisor = provision(&config).await;
    corrupt_mutation_journal(&config);

    let install = failure(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "install-after-corruption",
                vec!["touch must-not-install".into()],
            )))
            .await,
    );
    assert_eq!(install.code, "mutation_journal_corrupt");
    assert!(!config.home.join("must-not-install").exists());

    let workspace = failure(
        supervisor
            .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
                "workspace:wks_blocked:prepare:v1",
                "wks_blocked",
                "must-not-prepare",
                WorkspaceSource::Init,
            )))
            .await,
    );
    assert_eq!(workspace.code, "mutation_journal_corrupt");
    assert!(!config.home.join("must-not-prepare").exists());

    let home = failure(
        supervisor
            .dispatch(materialize_home_request(home_plan(
                "must-not-materialize",
                b"blocked",
            )))
            .await,
    );
    assert_eq!(home.code, "mutation_journal_corrupt");
    assert!(!config.agent_home("must-not-materialize").exists());
    cleanup(&config);
}

fn start_operation<T: serde::de::DeserializeOwned>(config: &RuntimeConfig, operation: &Operation) {
    crate::mutation_authority::admit(config, 1, operation).unwrap();
    assert!(matches!(
        mutation_journal::begin::<T>(config, operation).unwrap(),
        Begin::New(_)
    ));
}

fn staging_path(config: &RuntimeConfig, agent_id: &str, operation_id: &str) -> PathBuf {
    let digest = Sha256::digest(operation_id.as_bytes());
    let name = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    config
        .agent_home(agent_id)
        .parent()
        .unwrap()
        .join(format!(".home-staging-{name}"))
}

fn corrupt_mutation_journal(config: &RuntimeConfig) {
    let directory = config.state_root.join("runtime-mutations");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join(format!("{}.json", "0".repeat(64))),
        b"{broken",
    )
    .unwrap();
}
