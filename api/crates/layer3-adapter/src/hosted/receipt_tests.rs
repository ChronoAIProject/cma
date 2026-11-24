use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::{HostedAgentOpenSettings, HostedBackend, InMemoryHostedBackend};
use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::provider::ProviderSandboxRequest;
use crate::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame, WireOperation,
};
use crate::{
    AdapterError, AgentCloseRequest, AgentHomeRemoveRequest, AgentHomeRequest, AgentLaunch,
    AgentOpenRequest, AgentSlot, CheckpointHoldRequest, CheckpointRequest, ConversationChannel,
    ConversationUpdate, CredentialFrame, InstallRequest, RuntimeCredential, RuntimeHandle,
    RuntimeOptions, SandboxAdapter, SandboxSpec, WorkspaceReceipt, WorkspaceRemoveRequest,
    WorkspaceRequest, WorkspaceSetupRequest, WorkspaceSource, WorktreeReceipt, WorktreeRef,
    WorktreeRequest,
};

#[path = "reconnect_tests.rs"]
mod reconnect_tests;

#[path = "tool_confirmation_adapter_tests.rs"]
mod tool_confirmation_tests;

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
    HeldCheckpointToken,
    AuthorityFence,
}

enum RestartReply {
    Conflict,
    WrongConversation,
}

#[derive(Clone, Default)]
struct CorruptingBackend {
    inner: InMemoryHostedBackend,
    next: Arc<Mutex<Option<Corruption>>>,
    expected_reconnect: Arc<Mutex<Option<String>>>,
    opens: Arc<std::sync::atomic::AtomicUsize>,
    restart_reply: Arc<Mutex<Option<(crate::AgentRestartRuntime, RestartReply)>>>,
    opened_access: Arc<Mutex<Option<super::HostedAccessToolBinding>>>,
}

impl CorruptingBackend {
    fn corrupt_next(&self, corruption: Corruption) {
        *self.next.lock().expect("corruption lock") = Some(corruption);
    }

    fn corrupt_response(&self, response: &mut ResponseFrame) {
        let Some(corruption) = self.next.lock().expect("corruption lock").take() else {
            return;
        };
        if matches!(corruption, Corruption::AuthorityFence) {
            match response {
                ResponseFrame::AddWorktree(receipt) => receipt.authority_fence += 1,
                ResponseFrame::RemoveWorktree(receipt) => receipt.authority_fence += 1,
                ResponseFrame::MaterializeAgentHome(receipt) => receipt.authority_fence += 1,
                ResponseFrame::RemoveAgentHome(receipt) => receipt.authority_fence += 1,
                ResponseFrame::CloseAgent(receipt) => receipt.authority_fence += 1,
                _ => panic!("authority corruption did not match a fenced runtime response"),
            }
            return;
        }
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
                receipt.receipt.path.push_str("-renamed");
            }
            (Corruption::WorktreeRemovalIdentity, ResponseFrame::RemoveWorktree(receipt)) => {
                receipt.receipt.name.push_str("-other");
            }
            (Corruption::AgentHomePath, ResponseFrame::MaterializeAgentHome(receipt)) => {
                receipt.receipt.path.push_str("-other");
            }
            (Corruption::AgentCloseIdentity, ResponseFrame::CloseAgent(receipt)) => {
                receipt.receipt.agent_id = AgentSlot::new("agent-other");
            }
            (Corruption::AgentHomeRemovalIdentity, ResponseFrame::RemoveAgentHome(receipt)) => {
                receipt.receipt.agent_id = AgentSlot::new("agent-other");
            }
            (Corruption::RemovalIdentity, ResponseFrame::RemoveWorkspace(receipt)) => {
                receipt.workspace_id.push_str("-other");
            }
            (Corruption::CheckpointReference, ResponseFrame::Checkpoint(receipt)) => {
                receipt.object_reference = format!("renamed-{}", receipt.object_reference);
            }
            (Corruption::HeldCheckpointToken, ResponseFrame::CheckpointForTeardown(receipt)) => {
                receipt.writer_resume_token = "f".repeat(64)
            }
            (Corruption::AuthorityFence, _) => unreachable!(),
            _ => panic!("corruption did not match the runtime response"),
        }
    }

    fn corrupt_open_receipt(&self, receipt: &mut FencedReceipt<AgentOpenReceipt>) {
        let Some(corruption) = self.next.lock().expect("corruption lock").take() else {
            return;
        };
        match corruption {
            Corruption::AgentOpenIdentity => receipt.receipt.cwd.push_str("-other"),
            Corruption::AuthorityFence => receipt.authority_fence += 1,
            _ => panic!("corruption did not match the runtime open response"),
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
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some((expected, reply)) = self.restart_reply.lock().unwrap().take() {
            assert_eq!(launch.restart_runtime.as_ref(), Some(&expected));
            return match reply {
                RestartReply::Conflict => Err(AdapterError::OperationIdentityConflict(
                    "restart fixture conflict".into(),
                )),
                RestartReply::WrongConversation => {
                    let wrong = Some("another-conversation".into());
                    let channel = ConversationChannel {
                        input: crate::ConversationInputSender::spawn(1, |_| async { Ok(()) }),
                        conversation_id: wrong.clone(),
                    };
                    let receipt = AgentOpenReceipt {
                        operation_id: launch.request.operation_id,
                        request_sha256: launch.request.request_sha256,
                        agent_id: launch.request.agent_id,
                        cwd: launch.request.cwd,
                        home: launch.request.home,
                        conversation_id: wrong,
                    };
                    Ok((channel, FencedReceipt::new(launch.authority_fence, receipt)))
                }
            };
        }
        *self.opened_access.lock().unwrap() = settings.access_tool.clone();
        if let Some(expected) = self.expected_reconnect.lock().unwrap().as_deref() {
            assert_eq!(resume_conversation.as_deref(), Some(expected));
        }
        let (channel, mut receipt) = self
            .inner
            .open_agent(handle, launch, resume_conversation, settings, updates)
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

fn unavailable<T>(result: crate::Result<T>) {
    assert!(matches!(result, Err(AdapterError::Unavailable(_))));
}

async fn ready_runtime(
    adapter: &CodexAdapter,
    sandbox_id: &str,
    workspace: &str,
) -> (RuntimeHandle, WorkspaceReceipt) {
    let handle = adapter
        .provision(&SandboxSpec {
            sandbox_id: sandbox_id.into(),
            ..SandboxSpec::default()
        })
        .await
        .unwrap();
    adapter
        .run_install(
            &handle,
            &InstallRequest::new(format!("sandbox-install-v1:{workspace}"), Vec::new()),
        )
        .await
        .unwrap();
    adapter
        .deliver_credential(
            &handle,
            CredentialFrame {
                operation_id: format!("credential-{workspace}"),
                value: RuntimeCredential::new("opaque-runtime-credential"),
                custody_generation: Some(1),
                github_route: None,
            },
        )
        .await
        .unwrap();
    let workspace_id = format!("wks-{workspace}");
    let receipt = adapter
        .prepare_workspace(
            &handle,
            &WorkspaceRequest::new(
                format!("workspace:{workspace_id}:prepare:v1"),
                workspace_id,
                workspace,
                WorkspaceSource::Init,
            ),
        )
        .await
        .unwrap();
    (handle, receipt)
}

fn runtime_options(worktree: &WorktreeReceipt, agent_id: &str) -> RuntimeOptions {
    RuntimeOptions {
        working_directory: worktree.path.clone(),
        home_directory: format!("/home/cma/.cma/agents/{agent_id}/home"),
        slot: AgentSlot::new(agent_id),
        ..RuntimeOptions::default()
    }
}

#[tokio::test]
async fn reconnect_supplies_held_identity_without_changing_the_durable_open() {
    let backend = CorruptingBackend::default();
    let adapter = CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default());
    let (handle, workspace) = ready_runtime(&adapter, "sandbox-reconnect", "reconnect").await;
    let agent_id = "agent-reconnect";
    let worktree = adapter
        .add_worktree(
            &handle,
            1,
            &WorktreeRequest::new(
                format!("worktree:reconnect:{agent_id}:add:v1"),
                "reconnect",
                agent_id,
                workspace.resolved_commit,
            ),
        )
        .await
        .unwrap();
    let options = runtime_options(&worktree, agent_id);
    adapter
        .materialize_agent_home(
            &handle,
            1,
            &AgentHomeRequest::new(
                format!("agent:{agent_id}:home:create:v1"),
                options.clone(),
                "prompt",
                Vec::new(),
            ),
        )
        .await
        .unwrap();
    let open = AgentOpenRequest::new(format!("agent:{agent_id}:open:v1"), options, None);
    let (updates, _receiver) = mpsc::channel(1);
    let initial = adapter
        .open_agent(&handle, 1, &open, updates)
        .await
        .unwrap();
    let conversation = initial.conversation_id.unwrap();
    let original = backend.inner.snapshot(&handle).await.unwrap().agents;
    *backend.expected_reconnect.lock().unwrap() = Some(conversation.clone());
    let (updates, _receiver) = mpsc::channel(1);
    let attached = adapter
        .reconnect_agent(&handle, 2, &open, &conversation, updates)
        .await
        .unwrap();
    assert_eq!(
        attached.conversation_id.as_deref(),
        Some(conversation.as_str())
    );
    assert_eq!(
        backend.inner.snapshot(&handle).await.unwrap().agents,
        original
    );
    assert!(open.resume_conversation.is_none());
    assert!(open.has_valid_sha256());

    for invalid in ["", " ", "conversation\n", "other-conversation"] {
        *backend.expected_reconnect.lock().unwrap() = Some(invalid.into());
        let (updates, _receiver) = mpsc::channel(1);
        rejected(
            adapter
                .reconnect_agent(&handle, 3, &open, invalid, updates)
                .await,
        );
    }
    let resumed =
        AgentOpenRequest::new(open.operation_id.clone(), open.options, Some(conversation));
    let (updates, _receiver) = mpsc::channel(1);
    rejected(
        adapter
            .reconnect_agent(&handle, 4, &resumed, "different-conversation", updates)
            .await,
    );
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
                    github_route: None,
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
    rejected(adapter.add_worktree(&handle, 1, &add_worktree).await);
    let worktree = adapter
        .add_worktree(&handle, 1, &add_worktree)
        .await
        .unwrap();

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
    rejected(adapter.materialize_agent_home(&handle, 1, &home).await);
    adapter
        .materialize_agent_home(&handle, 1, &home)
        .await
        .unwrap();

    let open = AgentOpenRequest::new("agent:agent-1:open:v1", options.clone(), None);
    backend.corrupt_next(Corruption::AgentOpenIdentity);
    let (updates, _updates_rx) = mpsc::channel(1);
    rejected(adapter.open_agent(&handle, 1, &open, updates).await);
    let (updates, _updates_rx) = mpsc::channel(1);
    adapter
        .open_agent(&handle, 1, &open, updates)
        .await
        .unwrap();

    let close = AgentCloseRequest::new("agent:agent-1:close:v1", options.slot.clone());
    backend.corrupt_next(Corruption::AgentCloseIdentity);
    rejected(adapter.close_agent(&handle, 1, &close).await);
    adapter.close_agent(&handle, 1, &close).await.unwrap();
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
    rejected(adapter.remove_agent_home(&handle, 1, &remove_home).await);
    adapter
        .remove_agent_home(&handle, 1, &remove_home)
        .await
        .unwrap();

    let remove_worktree =
        WorktreeRef::new("worktree:receipt:agent-1:remove:v1", "receipt", "agent-1");
    backend.corrupt_next(Corruption::WorktreeRemovalIdentity);
    rejected(adapter.remove_worktree(&handle, 1, &remove_worktree).await);
    adapter
        .remove_worktree(&handle, 1, &remove_worktree)
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

    backend.corrupt_next(Corruption::HeldCheckpointToken);
    rejected(
        adapter
            .checkpoint_for_teardown(
                &handle,
                &CheckpointHoldRequest {
                    checkpoint: CheckpointRequest {
                        epoch: 8,
                        exclusions: Vec::new(),
                    },
                    writer_resume_token: "e".repeat(64),
                },
            )
            .await,
    );
    assert!(!backend.inner.snapshot(&handle).await.unwrap().writers_held);
}

#[tokio::test]
async fn higher_fence_retries_recover_lost_mutation_responses_without_duplicate_effects() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let (handle, workspace) =
        ready_runtime(&adapter, "sandbox-lost-fenced-responses", "lost").await;
    let agent_id = "agent-lost";
    let add = WorktreeRequest::new(
        format!("worktree:lost:{agent_id}:add:v1"),
        "lost",
        agent_id,
        workspace.resolved_commit,
    );

    backend.lose_next_response(WireOperation::AddWorktree);
    unavailable(adapter.add_worktree(&handle, 7, &add).await);
    let applied_worktree = backend
        .snapshot(&handle)
        .await
        .unwrap()
        .worktrees
        .get(&("lost".into(), agent_id.into()))
        .unwrap()
        .clone();
    let replayed_worktree = adapter.add_worktree(&handle, 8, &add).await.unwrap();
    assert_eq!(replayed_worktree, applied_worktree);
    assert_eq!(backend.snapshot(&handle).await.unwrap().worktrees.len(), 1);

    let options = runtime_options(&replayed_worktree, agent_id);
    let home = AgentHomeRequest::new(
        format!("agent:{agent_id}:home:create:v1"),
        options.clone(),
        "prompt",
        Vec::new(),
    );
    backend.lose_next_response(WireOperation::MaterializeAgentHome);
    unavailable(adapter.materialize_agent_home(&handle, 7, &home).await);
    let applied_home = backend
        .snapshot(&handle)
        .await
        .unwrap()
        .homes
        .get(&options.slot)
        .unwrap()
        .clone();
    adapter
        .materialize_agent_home(&handle, 8, &home)
        .await
        .unwrap();
    let snapshot = backend.snapshot(&handle).await.unwrap();
    assert_eq!(snapshot.homes.len(), 1);
    assert_eq!(snapshot.homes.get(&options.slot), Some(&applied_home));

    let open = AgentOpenRequest::new(format!("agent:{agent_id}:open:v1"), options.clone(), None);
    backend.lose_next_response(WireOperation::OpenAgent);
    let (lost_updates, _lost_updates_rx) = mpsc::channel(1);
    unavailable(adapter.open_agent(&handle, 7, &open, lost_updates).await);
    let applied_launch = backend
        .snapshot(&handle)
        .await
        .unwrap()
        .agents
        .get(&options.slot)
        .unwrap()
        .clone();
    let (replay_updates, _replay_updates_rx) = mpsc::channel(1);
    let channel = adapter
        .open_agent(&handle, 8, &open, replay_updates)
        .await
        .unwrap();
    assert_eq!(
        channel.conversation_id.as_deref(),
        Some("conversation-agent-lost")
    );
    let snapshot = backend.snapshot(&handle).await.unwrap();
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(snapshot.agents.get(&options.slot), Some(&applied_launch));

    let close = AgentCloseRequest::new(format!("agent:{agent_id}:close:v1"), options.slot.clone());
    adapter.close_agent(&handle, 9, &close).await.unwrap();
    let remove_home = AgentHomeRemoveRequest::new(
        format!("agent:{agent_id}:home:remove:v1"),
        options.slot.clone(),
    );
    adapter
        .remove_agent_home(&handle, 10, &remove_home)
        .await
        .unwrap();
    let remove_worktree = WorktreeRef::new(
        format!("worktree:lost:{agent_id}:remove:v1"),
        "lost",
        agent_id,
    );
    adapter
        .remove_worktree(&handle, 11, &remove_worktree)
        .await
        .unwrap();

    rejected(adapter.add_worktree(&handle, 10, &add).await);
    rejected(adapter.materialize_agent_home(&handle, 9, &home).await);
    let (stale_updates, _stale_updates_rx) = mpsc::channel(1);
    rejected(adapter.open_agent(&handle, 8, &open, stale_updates).await);
    let snapshot = backend.snapshot(&handle).await.unwrap();
    assert!(snapshot.worktrees.is_empty());
    assert!(snapshot.homes.is_empty());
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(snapshot.agents.get(&options.slot), Some(&applied_launch));
}

#[tokio::test]
async fn corrupt_outer_fences_are_rejected_and_higher_retries_do_not_repeat_effects() {
    let backend = CorruptingBackend::default();
    let adapter = CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default());
    let (handle, workspace) =
        ready_runtime(&adapter, "sandbox-corrupt-authority-fence", "fence").await;
    let agent_id = "agent-fence";
    let add = WorktreeRequest::new(
        format!("worktree:fence:{agent_id}:add:v1"),
        "fence",
        agent_id,
        workspace.resolved_commit,
    );
    let worktree = adapter.add_worktree(&handle, 1, &add).await.unwrap();
    backend.corrupt_next(Corruption::AuthorityFence);
    rejected(adapter.add_worktree(&handle, 2, &add).await);
    assert_eq!(
        adapter.add_worktree(&handle, 3, &add).await.unwrap(),
        worktree
    );
    assert_eq!(
        backend
            .inner
            .snapshot(&handle)
            .await
            .unwrap()
            .worktrees
            .len(),
        1
    );

    let options = runtime_options(&worktree, agent_id);
    let home = AgentHomeRequest::new(
        format!("agent:{agent_id}:home:create:v1"),
        options.clone(),
        "prompt",
        Vec::new(),
    );
    adapter
        .materialize_agent_home(&handle, 1, &home)
        .await
        .unwrap();
    backend.corrupt_next(Corruption::AuthorityFence);
    rejected(adapter.materialize_agent_home(&handle, 2, &home).await);
    adapter
        .materialize_agent_home(&handle, 3, &home)
        .await
        .unwrap();
    assert_eq!(
        backend.inner.snapshot(&handle).await.unwrap().homes.len(),
        1
    );

    let open = AgentOpenRequest::new(format!("agent:{agent_id}:open:v1"), options.clone(), None);
    let (initial_updates, _initial_updates_rx) = mpsc::channel(1);
    let initial = adapter
        .open_agent(&handle, 1, &open, initial_updates)
        .await
        .unwrap();
    backend.corrupt_next(Corruption::AuthorityFence);
    let (corrupt_updates, _corrupt_updates_rx) = mpsc::channel(1);
    rejected(adapter.open_agent(&handle, 2, &open, corrupt_updates).await);
    let (retry_updates, _retry_updates_rx) = mpsc::channel(1);
    let retried = adapter
        .open_agent(&handle, 3, &open, retry_updates)
        .await
        .unwrap();
    assert_eq!(retried.conversation_id, initial.conversation_id);
    assert_eq!(
        backend.inner.snapshot(&handle).await.unwrap().agents.len(),
        1
    );

    adapter
        .close_agent(
            &handle,
            4,
            &AgentCloseRequest::new(format!("agent:{agent_id}:close:v1"), options.slot.clone()),
        )
        .await
        .unwrap();
    adapter
        .remove_agent_home(
            &handle,
            5,
            &AgentHomeRemoveRequest::new(format!("agent:{agent_id}:home:remove:v1"), options.slot),
        )
        .await
        .unwrap();
    adapter
        .remove_worktree(
            &handle,
            6,
            &WorktreeRef::new(
                format!("worktree:fence:{agent_id}:remove:v1"),
                "fence",
                agent_id,
            ),
        )
        .await
        .unwrap();
}
