use super::*;
use crate::provider::ProviderSandboxRequest;
use crate::wire::{
    AgentHomeReceipt, AgentOpenReceipt, AgentResourcesReceipt, FencedReceipt, FencedRequest,
    RequestFrame, ResponseFrame,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct BatchBackend {
    memory: InMemoryHostedBackend,
    corrupt: AtomicUsize,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl HostedBackend for BatchBackend {
    async fn provision(&self, request: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        self.memory.provision(request).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        let RequestFrame::PrepareAgentResources(frame) = request else {
            return self.memory.exchange(handle, request).await;
        };
        self.calls.fetch_add(1, Ordering::SeqCst);
        let plan = frame.request;
        let mut reply = FencedReceipt::new(
            frame.authority_fence,
            AgentResourcesReceipt {
                worktree: WorktreeReceipt {
                    operation_id: plan.worktree.operation_id,
                    request_sha256: plan.worktree.request_sha256,
                    workspace: plan.worktree.workspace.clone(),
                    name: plan.worktree.name.clone(),
                    path: format!(
                        "/home/cma/{}/.worktrees/{}",
                        plan.worktree.workspace, plan.worktree.name
                    ),
                    branch: format!("cma/{}", plan.worktree.name),
                    base_commit: plan.worktree.base,
                },
                home: AgentHomeReceipt {
                    operation_id: plan.home.operation_id,
                    request_sha256: plan.home.request_sha256,
                    path: format!("/home/cma/.cma/agents/{}/home", plan.home.agent_id.as_str()),
                    agent_id: plan.home.agent_id,
                },
            },
        );
        match self.corrupt.load(Ordering::SeqCst) {
            0 => {}
            1 => reply.authority_fence += 1,
            2 => reply.receipt.worktree.operation_id.push('x'),
            3 => reply.receipt.worktree.request_sha256 = "b".repeat(64),
            4 => reply.receipt.worktree.path.push('x'),
            5 => reply.receipt.worktree.branch.push('x'),
            6 => reply.receipt.worktree.base_commit = "invalid".into(),
            7 => reply.receipt.home.operation_id.push('x'),
            8 => reply.receipt.home.request_sha256 = "b".repeat(64),
            9 => reply.receipt.home.agent_id = AgentSlot::new("other"),
            10 => reply.receipt.home.path.push('x'),
            _ => unreachable!(),
        }
        Ok(ResponseFrame::PrepareAgentResources(reply))
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("preparation cannot open a process")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> crate::Result<()> {
        panic!("preparation cannot delete a runtime")
    }
}

async fn fixture() -> (
    CodexAdapter,
    Arc<BatchBackend>,
    RuntimeHandle,
    WorktreeRequest,
    AgentHomeRequest,
) {
    let backend = Arc::new(BatchBackend::default());
    let adapter = CodexAdapter::new(backend.clone(), CodexSandboxSpec::default());
    let handle = adapter
        .provision(&SandboxSpec {
            sandbox_id: "batch-fixture".into(),
            ..SandboxSpec::default()
        })
        .await
        .unwrap();
    let tree = WorktreeRequest::new("worktree:repo:wt-7:add:v1", "repo", "wt-7", "a".repeat(40));
    let home = AgentHomeRequest::new(
        "agent:agent-7:home:create:v1",
        options(false, PermissionLevel::Autonomous, ReasoningEffort::High),
        "profile",
        vec![],
    );
    (adapter, backend, handle, tree, home)
}

#[tokio::test]
async fn resource_batch_outer_adapter_validates_each_receipt_without_resending() {
    let (adapter, backend, handle, tree, home) = fixture().await;
    assert_eq!(
        adapter
            .prepare_agent_resources(&handle, 7, &tree, &home)
            .await
            .unwrap()
            .path,
        home.options.working_directory
    );
    for corrupt in 1..=10 {
        backend.corrupt.store(corrupt, Ordering::SeqCst);
        assert!(
            adapter
                .prepare_agent_resources(&handle, 7, &tree, &home)
                .await
                .is_err(),
            "corruption {corrupt}"
        );
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            corrupt + 1,
            "no speculative retry or fallback"
        );
    }
}

#[tokio::test]
async fn resource_batch_invalid_identity_or_worktree_never_reaches_runtime() {
    let (adapter, backend, handle, tree, home) = fixture().await;
    for invalid in 0..4 {
        let mut tree = tree.clone();
        let mut home = home.clone();
        let mut fence = 7;
        match invalid {
            0 => fence = 0,
            1 => tree.request_sha256 = "b".repeat(64),
            2 => home.request_sha256 = "b".repeat(64),
            3 => {
                home.options.working_directory = "/home/cma/repo/.worktrees/other".into();
                home = AgentHomeRequest::new(
                    home.operation_id,
                    home.options,
                    home.system_prompt,
                    home.skills,
                );
            }
            _ => unreachable!(),
        }
        assert!(adapter
            .prepare_agent_resources(&handle, fence, &tree, &home)
            .await
            .is_err());
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}
