use super::HostedBackend;
use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::wire::{
    AgentCloseReceipt, AgentHomeRemoveReceipt, FencedReceipt, FencedRequest, RequestFrame,
    ResponseFrame,
};
use crate::{
    AdapterError, AgentCloseRequest, AgentHomeRemoveRequest, AgentHomeRequest, AgentOpenRequest,
    AgentSlot, CredentialFrame, InstallRequest, RuntimeCredential, RuntimeOptions, SandboxAdapter,
    SandboxSpec, WorkspaceRequest, WorkspaceSource, WorktreeRef, WorktreeRemoveReceipt,
    WorktreeRequest,
};

async fn fixture() -> (
    CodexAdapter,
    super::InMemoryHostedBackend,
    crate::RuntimeHandle,
) {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let handle = adapter
        .provision(&SandboxSpec {
            sandbox_id: "sandbox-absent-cleanup".into(),
            ..SandboxSpec::default()
        })
        .await
        .unwrap();
    adapter
        .run_install(
            &handle,
            &InstallRequest::new("sandbox-install-v1:absent-cleanup", Vec::new()),
        )
        .await
        .unwrap();
    adapter
        .deliver_credential(
            &handle,
            CredentialFrame {
                operation_id: "credential-absent-cleanup".into(),
                value: RuntimeCredential::new("test-credential"),
                custody_generation: Some(1),
                github_route: None,
            },
        )
        .await
        .unwrap();
    (adapter, backend, handle)
}

fn fenced<T>(request: T) -> FencedRequest<T> {
    FencedRequest::new(1, request)
}

fn receipt<T>(value: T) -> FencedReceipt<T> {
    FencedReceipt::new(1, value)
}

#[tokio::test]
async fn absent_cleanup_targets_return_exact_receipts_and_replay() {
    let (_adapter, backend, handle) = fixture().await;
    let agent_id = AgentSlot::new("agent-absent");

    let close = AgentCloseRequest::new("agent:agent-absent:close:v1", agent_id.clone());
    let expected_close = ResponseFrame::CloseAgent(receipt(AgentCloseReceipt {
        operation_id: close.operation_id.clone(),
        request_sha256: close.request_sha256.clone(),
        agent_id: agent_id.clone(),
    }));
    assert_eq!(
        backend
            .exchange(&handle, RequestFrame::CloseAgent(fenced(close.clone())))
            .await
            .unwrap(),
        expected_close
    );
    assert_eq!(
        backend
            .exchange(&handle, RequestFrame::CloseAgent(fenced(close)))
            .await
            .unwrap(),
        expected_close
    );

    let remove_home =
        AgentHomeRemoveRequest::new("agent:agent-absent:home:remove:v1", agent_id.clone());
    let expected_home = ResponseFrame::RemoveAgentHome(receipt(AgentHomeRemoveReceipt {
        operation_id: remove_home.operation_id.clone(),
        request_sha256: remove_home.request_sha256.clone(),
        agent_id,
    }));
    assert_eq!(
        backend
            .exchange(
                &handle,
                RequestFrame::RemoveAgentHome(fenced(remove_home.clone())),
            )
            .await
            .unwrap(),
        expected_home
    );
    assert_eq!(
        backend
            .exchange(&handle, RequestFrame::RemoveAgentHome(fenced(remove_home)))
            .await
            .unwrap(),
        expected_home
    );

    let remove_worktree = WorktreeRef::new(
        "worktree:workspace-absent:agent-absent:remove:v1",
        "workspace-absent",
        "agent-absent",
    );
    let expected_worktree = ResponseFrame::RemoveWorktree(receipt(WorktreeRemoveReceipt {
        operation_id: remove_worktree.operation_id.clone(),
        request_sha256: remove_worktree.request_sha256.clone(),
        workspace: remove_worktree.workspace.clone(),
        name: remove_worktree.name.clone(),
    }));
    assert_eq!(
        backend
            .exchange(
                &handle,
                RequestFrame::RemoveWorktree(fenced(remove_worktree.clone())),
            )
            .await
            .unwrap(),
        expected_worktree
    );
    assert_eq!(
        backend
            .exchange(
                &handle,
                RequestFrame::RemoveWorktree(fenced(remove_worktree)),
            )
            .await
            .unwrap(),
        expected_worktree
    );
}

#[tokio::test]
async fn absent_cleanup_does_not_bypass_identity_or_live_resource_checks() {
    let (adapter, backend, handle) = fixture().await;
    let invalid = AgentCloseRequest::new(
        "agent:another-agent:close:v1",
        AgentSlot::new("agent-absent"),
    );
    assert!(matches!(
        backend
            .exchange(&handle, RequestFrame::CloseAgent(fenced(invalid)))
            .await,
        Err(AdapterError::Rejected(_))
    ));
    let invalid = AgentCloseRequest::new("agent:.hidden:close:v1", AgentSlot::new(".hidden"));
    assert!(matches!(
        backend
            .exchange(&handle, RequestFrame::CloseAgent(fenced(invalid)))
            .await,
        Err(AdapterError::Rejected(_))
    ));
    let invalid = AgentHomeRemoveRequest::new(
        "agent:another-agent:home:remove:v1",
        AgentSlot::new("agent-absent"),
    );
    assert!(matches!(
        backend
            .exchange(&handle, RequestFrame::RemoveAgentHome(fenced(invalid)))
            .await,
        Err(AdapterError::Rejected(_))
    ));
    let invalid = WorktreeRef::new(
        "worktree:cleanup-guard:another-agent:remove:v1",
        "cleanup-guard",
        "agent-live",
    );
    assert!(matches!(
        backend
            .exchange(&handle, RequestFrame::RemoveWorktree(fenced(invalid)))
            .await,
        Err(AdapterError::Rejected(_))
    ));

    let workspace = adapter
        .prepare_workspace(
            &handle,
            &WorkspaceRequest::new(
                "workspace:wks_cleanup_guard:prepare:v1",
                "wks_cleanup_guard",
                "cleanup-guard",
                WorkspaceSource::Init,
            ),
        )
        .await
        .unwrap();
    let worktree = adapter
        .add_worktree(
            &handle,
            1,
            &WorktreeRequest::new(
                "worktree:cleanup-guard:agent-live:add:v1",
                "cleanup-guard",
                "agent-live",
                workspace.resolved_commit,
            ),
        )
        .await
        .unwrap();
    let options = RuntimeOptions {
        working_directory: worktree.path,
        home_directory: "/home/cma/.cma/agents/agent-live/home".into(),
        slot: AgentSlot::new("agent-live"),
        ..RuntimeOptions::default()
    };
    adapter
        .materialize_agent_home(
            &handle,
            1,
            &AgentHomeRequest::new(
                "agent:agent-live:home:create:v1",
                options.clone(),
                "prompt",
                Vec::new(),
            ),
        )
        .await
        .unwrap();
    let (updates, _updates_rx) = tokio::sync::mpsc::channel(1);
    adapter
        .open_agent(
            &handle,
            1,
            &AgentOpenRequest::new("agent:agent-live:open:v1", options.clone(), None),
            updates,
        )
        .await
        .unwrap();

    assert!(matches!(
        adapter
            .remove_agent_home(
                &handle,
                1, &AgentHomeRemoveRequest::new(
                    "agent:agent-live:home:remove:v1",
                    options.slot.clone(),
                ),
            )
            .await,
        Err(AdapterError::Rejected(detail)) if detail.contains("agent_running")
    ));
    assert!(matches!(
        adapter
            .remove_worktree(
                &handle,
                1, &WorktreeRef::new(
                    "worktree:cleanup-guard:agent-live:remove:v1",
                    "cleanup-guard",
                    "agent-live",
                ),
            )
            .await,
        Err(AdapterError::Rejected(detail)) if detail.contains("worktree_agent_open")
    ));
}
