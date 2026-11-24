use super::*;

pub(super) fn add_worktree_request(request: WorktreeRequest) -> RequestFrame {
    RequestFrame::AddWorktree(FencedRequest::new(1, request))
}

pub(super) fn remove_worktree_request(request: WorktreeRef) -> RequestFrame {
    RequestFrame::RemoveWorktree(FencedRequest::new(1, request))
}

pub(super) fn materialize_home_request(request: AgentHomePlan) -> RequestFrame {
    RequestFrame::MaterializeAgentHome(FencedRequest::new(1, request))
}

pub(super) fn remove_home_request(request: AgentHomeRemoveRequest) -> RequestFrame {
    RequestFrame::RemoveAgentHome(FencedRequest::new(1, request))
}

pub(super) fn open_agent_request(request: AgentLaunch) -> RequestFrame {
    RequestFrame::OpenAgent(FencedRequest::new(1, request))
}

pub(super) fn close_agent_request(request: AgentCloseRequest) -> RequestFrame {
    RequestFrame::CloseAgent(FencedRequest::new(1, request))
}

pub(super) fn cleanup(config: &RuntimeConfig) {
    let _ = std::fs::remove_dir_all(&config.home);
    let _ = std::fs::remove_dir_all(&config.checkpoint_root);
    let _ = std::fs::remove_dir_all(&config.state_root);
}

pub(super) fn failure(response: ResponseFrame) -> WireFailure {
    match response {
        ResponseFrame::Failure(failure) => failure,
        other => panic!("expected failure response, got {other:?}"),
    }
}

pub(super) async fn credential(supervisor: &Supervisor) {
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

pub(super) async fn prepare_workspace(supervisor: &Supervisor) -> WorkspaceReceipt {
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

pub(super) async fn add_worktree(
    supervisor: &Supervisor,
    base: String,
) -> (WorktreeRequest, WorktreeReceipt) {
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

pub(super) fn home_plan(agent_id: &str, contents: &[u8]) -> AgentHomePlan {
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

pub(super) fn launch(
    config: &RuntimeConfig,
    worktree: &WorktreeReceipt,
    script: &str,
) -> AgentLaunch {
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
