use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::{HostedBackend, InMemoryHostedBackend};
use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::provider::ProviderSandboxRequest;
use crate::wire::{AgentOpenReceipt, RequestFrame, ResponseFrame};
use crate::{
    AdapterError, AgentCloseRequest, AgentHomeRemoveRequest, AgentHomeRequest, AgentLaunch,
    AgentOpenRequest, AgentSlot, CheckpointRequest, ConversationChannel, ConversationUpdate,
    CredentialFrame, InstallRequest, RuntimeCredential, RuntimeHandle, RuntimeOptions,
    SandboxAdapter, SandboxSpec, WorkspaceRemoveRequest, WorkspaceRequest, WorkspaceSetupRequest,
    WorkspaceSource, WorktreeRef, WorktreeRequest,
};

#[derive(Clone, Copy)]
enum Corruption {
    Credential,
    WorkspacePath,
    WorkspaceCommit,
    SetupHash,
    WorktreePath,
    WorktreeRemovalIdentity,
    AgentHomePath,
    AgentOpenIdentity,
    AgentCloseIdentity,
    AgentHomeRemovalIdentity,
    RemovalIdentity,
    CheckpointReference,
}

#[derive(Clone, Default)]
struct CorruptingBackend {
    inner: InMemoryHostedBackend,
    next: Arc<Mutex<Option<Corruption>>>,
}

impl CorruptingBackend {
    fn corrupt_next(&self, corruption: Corruption) {
        *self.next.lock().expect("corruption lock") = Some(corruption);
    }

    fn corrupt_response(&self, response: &mut ResponseFrame) {
        let Some(corruption) = self.next.lock().expect("corruption lock").take() else {
            return;
        };
        match (corruption, response) {
            (Corruption::Credential, ResponseFrame::Credential(receipt)) => {
                receipt.installed = false;
            }
            (Corruption::WorkspacePath, ResponseFrame::PrepareWorkspace(receipt)) => {
                receipt.path.push_str("-renamed");
            }
            (Corruption::WorkspaceCommit, ResponseFrame::PrepareWorkspace(receipt)) => {
                receipt.resolved_commit = "A".repeat(64);
            }
            (Corruption::SetupHash, ResponseFrame::WorkspaceSetup(receipt)) => {
                receipt.commands[0].stdout_sha256 = "A".repeat(64);
            }
            (Corruption::WorktreePath, ResponseFrame::AddWorktree(receipt)) => {
                receipt.path.push_str("-renamed");
            }
            (Corruption::WorktreeRemovalIdentity, ResponseFrame::RemoveWorktree(receipt)) => {
                receipt.name.push_str("-other");
            }
            (Corruption::AgentHomePath, ResponseFrame::MaterializeAgentHome(receipt)) => {
                receipt.path.push_str("-other");
            }
            (Corruption::AgentCloseIdentity, ResponseFrame::CloseAgent(receipt)) => {
                receipt.agent_id = AgentSlot::new("agent-other");
            }
            (Corruption::AgentHomeRemovalIdentity, ResponseFrame::RemoveAgentHome(receipt)) => {
                receipt.agent_id = AgentSlot::new("agent-other");
            }
            (Corruption::RemovalIdentity, ResponseFrame::RemoveWorkspace(receipt)) => {
                receipt.workspace_id.push_str("-other");
            }
            (Corruption::CheckpointReference, ResponseFrame::Checkpoint(receipt)) => {
                receipt.object_reference = format!("renamed-{}", receipt.object_reference);
            }
            _ => panic!("corruption did not match the runtime response"),
        }
    }

    fn corrupt_open_receipt(&self, receipt: &mut AgentOpenReceipt) {
        let Some(corruption) = self.next.lock().expect("corruption lock").take() else {
            return;
        };
        if matches!(corruption, Corruption::AgentOpenIdentity) {
            receipt.cwd.push_str("-other");
        } else {
            panic!("corruption did not match the runtime open response");
        }
    }
}

#[async_trait::async_trait]
impl HostedBackend for CorruptingBackend {
    async fn provision(&self, request: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        self.inner.provision(request).await
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        let mut response = self.inner.exchange(handle, request).await?;
        self.corrupt_response(&mut response);
        Ok(response)
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: AgentLaunch,
        resume_conversation: Option<String>,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, AgentOpenReceipt)> {
        let (channel, mut receipt) = self
            .inner
            .open_agent(handle, launch, resume_conversation, updates)
            .await?;
        self.corrupt_open_receipt(&mut receipt);
        Ok((channel, receipt))
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        self.inner.teardown(handle).await
    }
}

fn rejected<T>(result: crate::Result<T>) {
    assert!(matches!(result, Err(AdapterError::Rejected(_))));
}

#[tokio::test]
async fn hosted_adapter_rejects_semantically_corrupt_runtime_receipts() {
    let backend = CorruptingBackend::default();
    let adapter = CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default());
    let spec = SandboxSpec {
        sandbox_id: "sandbox-receipt-validation".into(),
        ..SandboxSpec::default()
    };
    let handle = adapter.provision(&spec).await.unwrap();
    adapter
        .run_install(
            &handle,
            &InstallRequest::new("sandbox-install-v1:receipt-validation", Vec::new()),
        )
        .await
        .unwrap();

    backend.corrupt_next(Corruption::Credential);
    rejected(
        adapter
            .deliver_credential(
                &handle,
                CredentialFrame {
                    operation_id: "credential-receipt-validation".into(),
                    value: RuntimeCredential::new("opaque-runtime-credential"),
                    custody_generation: Some(1),
                },
            )
            .await,
    );

    let workspace_request = WorkspaceRequest::new(
        "workspace:wks-receipt:prepare:v1",
        "wks-receipt",
        "receipt",
        WorkspaceSource::Init,
    );
    backend.corrupt_next(Corruption::WorkspacePath);
    rejected(adapter.prepare_workspace(&handle, &workspace_request).await);
    let workspace = adapter
        .prepare_workspace(&handle, &workspace_request)
        .await
        .unwrap();

    backend.corrupt_next(Corruption::WorkspaceCommit);
    rejected(
        adapter
            .prepare_workspace(
                &handle,
                &WorkspaceRequest::new(
                    "workspace:wks-commit:prepare:v1",
                    "wks-commit",
                    "commit",
                    WorkspaceSource::Init,
                ),
            )
            .await,
    );

    backend.corrupt_next(Corruption::SetupHash);
    rejected(
        adapter
            .run_workspace_setup(
                &handle,
                &WorkspaceSetupRequest::new(
                    "workspace:wks-receipt:setup:v1",
                    "wks-receipt",
                    "receipt",
                    vec!["true".into()],
                ),
            )
            .await,
    );

    let add_worktree = WorktreeRequest::new(
        "worktree:receipt:agent-1:add:v1",
        "receipt",
        "agent-1",
        workspace.resolved_commit,
    );
    backend.corrupt_next(Corruption::WorktreePath);
    rejected(adapter.add_worktree(&handle, &add_worktree).await);
    let worktree = adapter.add_worktree(&handle, &add_worktree).await.unwrap();

    let options = RuntimeOptions {
        working_directory: worktree.path,
        home_directory: "/home/cma/.cma/agents/agent-1/home".into(),
        slot: AgentSlot::new("agent-1"),
        ..RuntimeOptions::default()
    };
    let home = AgentHomeRequest::new(
        "agent:agent-1:home:create:v1",
        options.clone(),
        "prompt",
        Vec::new(),
    );
    backend.corrupt_next(Corruption::AgentHomePath);
    rejected(adapter.materialize_agent_home(&handle, &home).await);
    adapter
        .materialize_agent_home(&handle, &home)
        .await
        .unwrap();

    let open = AgentOpenRequest::new("agent:agent-1:open:v1", options.clone(), None);
    backend.corrupt_next(Corruption::AgentOpenIdentity);
    let (updates, _updates_rx) = mpsc::channel(1);
    rejected(adapter.open_agent(&handle, &open, updates).await);
    let (updates, _updates_rx) = mpsc::channel(1);
    adapter.open_agent(&handle, &open, updates).await.unwrap();

    let close = AgentCloseRequest::new("agent:agent-1:close:v1", options.slot.clone());
    backend.corrupt_next(Corruption::AgentCloseIdentity);
    rejected(adapter.close_agent(&handle, &close).await);
    adapter.close_agent(&handle, &close).await.unwrap();
    assert!(backend
        .inner
        .snapshot(&handle)
        .await
        .unwrap()
        .homes
        .contains_key(&options.slot));

    let remove_home =
        AgentHomeRemoveRequest::new("agent:agent-1:home:remove:v1", options.slot.clone());
    backend.corrupt_next(Corruption::AgentHomeRemovalIdentity);
    rejected(adapter.remove_agent_home(&handle, &remove_home).await);
    adapter
        .remove_agent_home(&handle, &remove_home)
        .await
        .unwrap();

    let remove_worktree =
        WorktreeRef::new("worktree:receipt:agent-1:remove:v1", "receipt", "agent-1");
    backend.corrupt_next(Corruption::WorktreeRemovalIdentity);
    rejected(adapter.remove_worktree(&handle, &remove_worktree).await);
    adapter
        .remove_worktree(&handle, &remove_worktree)
        .await
        .unwrap();

    backend.corrupt_next(Corruption::RemovalIdentity);
    rejected(
        adapter
            .remove_workspace(
                &handle,
                &WorkspaceRemoveRequest::new(
                    "workspace:wks-commit:remove:v1",
                    "wks-commit",
                    "commit",
                ),
            )
            .await,
    );

    backend.corrupt_next(Corruption::CheckpointReference);
    rejected(
        adapter
            .checkpoint(
                &handle,
                &CheckpointRequest {
                    epoch: 7,
                    exclusions: Vec::new(),
                },
            )
            .await,
    );
}
