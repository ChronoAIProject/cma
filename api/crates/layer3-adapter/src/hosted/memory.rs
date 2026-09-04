use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, Mutex};

use super::deployment::provider_request_identity;
use super::HostedBackend;
use crate::provider::ProviderSandboxRequest;
use crate::wire::{
    Acknowledgement, CredentialReceipt, RequestFrame, ResponseFrame, WireFailure, WireOperation,
};
use crate::{
    AdapterError, AgentHomePlan, AgentLaunch, AgentSlot, CheckpointReceipt, CommandReceipt,
    ConversationChannel, ConversationInputSender, ConversationSnapshot, ConversationUpdate,
    InstallReceipt, ResourceReading, RuntimeHandle, SetupReceipt, WorkspaceReceipt,
    WorkspaceRemoveReceipt, WorkspaceSource, WorktreeReceipt, WriterState,
};

/// Observable in-memory state used by Layer 2 lifecycle tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InMemoryRuntimeSnapshot {
    pub request: ProviderSandboxRequest,
    pub credential_delivered: bool,
    pub workspaces: BTreeMap<String, WorkspaceReceipt>,
    pub worktrees: BTreeMap<(String, String), WorktreeReceipt>,
    pub homes: BTreeMap<AgentSlot, AgentHomePlan>,
    pub agents: BTreeMap<AgentSlot, AgentLaunch>,
    pub writer_fence_captures: u64,
    pub restored_object: Option<String>,
}

#[derive(Debug, Clone)]
struct MemoryAgent {
    launch: AgentLaunch,
    conversation_id: String,
    running: bool,
}

#[derive(Debug, Clone)]
struct MemoryRuntime {
    request: ProviderSandboxRequest,
    install: Option<((String, String), InstallReceipt)>,
    credential_delivered: bool,
    workspaces: BTreeMap<String, WorkspaceReceipt>,
    workspace_operations: BTreeMap<String, (String, ResponseFrame)>,
    worktrees: BTreeMap<(String, String), WorktreeReceipt>,
    homes: BTreeMap<AgentSlot, AgentHomePlan>,
    agents: BTreeMap<AgentSlot, MemoryAgent>,
    writer_fence_captures: u64,
    restored_object: Option<String>,
}

impl MemoryRuntime {
    fn snapshot(&self) -> InMemoryRuntimeSnapshot {
        InMemoryRuntimeSnapshot {
            request: self.request.clone(),
            credential_delivered: self.credential_delivered,
            workspaces: self.workspaces.clone(),
            worktrees: self.worktrees.clone(),
            homes: self.homes.clone(),
            agents: self
                .agents
                .iter()
                .map(|(slot, agent)| (slot.clone(), agent.launch.clone()))
                .collect(),
            writer_fence_captures: self.writer_fence_captures,
            restored_object: self.restored_object.clone(),
        }
    }
}

/// Deterministic hosted backend with no filesystem or network effects.
#[derive(Clone, Default)]
pub struct InMemoryHostedBackend {
    next_handle: Arc<AtomicU64>,
    runtimes: Arc<Mutex<BTreeMap<RuntimeHandle, MemoryRuntime>>>,
    requests: Arc<Mutex<Vec<(RuntimeHandle, RequestFrame)>>>,
    failures: Arc<StdMutex<VecDeque<WireFailure>>>,
    corrupt_install_receipt: Arc<AtomicBool>,
    corrupt_capture_digest: Arc<AtomicBool>,
}

impl InMemoryHostedBackend {
    pub async fn snapshot(&self, handle: &RuntimeHandle) -> crate::Result<InMemoryRuntimeSnapshot> {
        self.runtimes
            .lock()
            .await
            .get(handle)
            .map(MemoryRuntime::snapshot)
            .ok_or_else(|| AdapterError::Unavailable(format!("unknown runtime {}", handle.0)))
    }

    pub async fn requests(&self) -> Vec<(RuntimeHandle, RequestFrame)> {
        self.requests.lock().await.clone()
    }

    /// Return one typed runtime failure the next time this operation is sent.
    pub fn fail_next(
        &self,
        operation: WireOperation,
        code: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.failures
            .lock()
            .expect("in-memory failure queue lock")
            .push_back(WireFailure {
                operation,
                code: code.into(),
                detail: detail.into(),
                retryable: false,
            });
    }

    /// Make the next successful install reply carry an invalid receipt.
    pub fn corrupt_next_install_receipt(&self) {
        self.corrupt_install_receipt.store(true, Ordering::Relaxed);
    }

    /// Make the next captured archive disagree with its advertised digest.
    pub fn corrupt_next_capture_digest(&self) {
        self.corrupt_capture_digest.store(true, Ordering::Relaxed);
    }

    fn queued_failure(&self, operation: WireOperation) -> Option<WireFailure> {
        let mut failures = self.failures.lock().expect("in-memory failure queue lock");
        let index = failures
            .iter()
            .position(|failure| failure.operation == operation)?;
        failures.remove(index)
    }
}

#[async_trait::async_trait]
impl HostedBackend for InMemoryHostedBackend {
    async fn provision(&self, request: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        let identity = provider_request_identity(&request)?;
        let mut runtimes = self.runtimes.lock().await;
        if let Some((handle, runtime)) = runtimes.iter().find(|(_, runtime)| {
            provider_request_identity(&runtime.request)
                .is_ok_and(|existing| existing.sandbox_id == identity.sandbox_id)
        }) {
            return if provider_request_identity(&runtime.request)? == identity {
                Ok(handle.clone())
            } else {
                Err(AdapterError::Rejected(
                    "sandbox identity belongs to another specification".into(),
                ))
            };
        }
        let id = self.next_handle.fetch_add(1, Ordering::Relaxed) + 1;
        let handle = RuntimeHandle(format!("runtime-{id}"));
        runtimes.insert(
            handle.clone(),
            MemoryRuntime {
                request,
                install: None,
                credential_delivered: false,
                workspaces: BTreeMap::new(),
                workspace_operations: BTreeMap::new(),
                worktrees: BTreeMap::new(),
                homes: BTreeMap::new(),
                agents: BTreeMap::new(),
                writer_fence_captures: 0,
                restored_object: None,
            },
        );
        Ok(handle)
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        let operation = request.operation();
        self.requests
            .lock()
            .await
            .push((handle.clone(), request.clone()));
        if let Some(failure) = self.queued_failure(operation) {
            return Ok(ResponseFrame::Failure(failure));
        }

        let mut runtimes = self.runtimes.lock().await;
        let runtime = runtimes
            .get_mut(handle)
            .ok_or_else(|| AdapterError::Unavailable(format!("unknown runtime {}", handle.0)))?;
        match request {
            RequestFrame::Install(request) => {
                if request.operation_id.trim().is_empty() || !request.has_valid_sha256() {
                    return refused(operation, "install_request_invalid");
                }
                let identity = (request.operation_id.clone(), request.request_sha256.clone());
                if let Some((completed, receipt)) = &runtime.install {
                    return if completed == &identity {
                        Ok(ResponseFrame::Install(receipt.clone()))
                    } else {
                        refused(operation, "install_replay_mismatch")
                    };
                }
                let receipt = InstallReceipt {
                    operation_id: request.operation_id.clone(),
                    request_sha256: request.request_sha256,
                    commands: successful_commands(&request.commands),
                };
                runtime.install = Some((identity, receipt.clone()));
                let mut response = receipt;
                if self.corrupt_install_receipt.swap(false, Ordering::Relaxed) {
                    if let Some(first) = response.commands.first_mut() {
                        first.index = u32::MAX;
                    } else {
                        response.commands.push(CommandReceipt {
                            index: 0,
                            exit_code: 0,
                            stdout_sha256: digest(&[]),
                            stderr_sha256: digest(&[]),
                        });
                    }
                }
                Ok(ResponseFrame::Install(response))
            }
            RequestFrame::Credential(frame) => {
                if runtime.install.is_none() {
                    return refused(operation, "install_not_completed");
                }
                runtime.credential_delivered = true;
                Ok(ResponseFrame::Credential(CredentialReceipt {
                    operation_id: frame.operation_id,
                    installed: true,
                }))
            }
            RequestFrame::PrepareWorkspace(request) => {
                require_credential(runtime, operation)?;
                if !request.has_valid_sha256() || request.operation_id.trim().is_empty() {
                    return refused(operation, "workspace_request_invalid");
                }
                if let Some(response) = replay_workspace_operation(
                    runtime,
                    &request.operation_id,
                    &request.request_sha256,
                    operation,
                )? {
                    return Ok(response);
                }
                if !safe_name(&request.name) || runtime.workspaces.contains_key(&request.name) {
                    return refused(operation, "invalid_or_existing_workspace");
                }
                let commit_source = match &request.source {
                    WorkspaceSource::Clone { url, reference } => {
                        format!("{url}:{}", reference.as_deref().unwrap_or("default"))
                    }
                    WorkspaceSource::Init => format!("init:{}", request.name),
                };
                let receipt = WorkspaceReceipt {
                    operation_id: request.operation_id.clone(),
                    request_sha256: request.request_sha256.clone(),
                    workspace_id: request.workspace_id.clone(),
                    name: request.name.clone(),
                    path: format!("/home/cma/{}", request.name),
                    resolved_commit: digest(commit_source.as_bytes()),
                };
                runtime.workspaces.insert(request.name, receipt.clone());
                let response = ResponseFrame::PrepareWorkspace(receipt);
                remember_workspace_operation(
                    runtime,
                    request.operation_id,
                    request.request_sha256,
                    response.clone(),
                );
                Ok(response)
            }
            RequestFrame::WorkspaceSetup(request) => {
                require_credential(runtime, operation)?;
                if !request.has_valid_sha256() || request.operation_id.trim().is_empty() {
                    return refused(operation, "workspace_request_invalid");
                }
                if let Some(response) = replay_workspace_operation(
                    runtime,
                    &request.operation_id,
                    &request.request_sha256,
                    operation,
                )? {
                    return Ok(response);
                }
                let Some(workspace) = runtime.workspaces.get(&request.name) else {
                    return refused(operation, "workspace_not_found");
                };
                if workspace.workspace_id != request.workspace_id {
                    return refused(operation, "workspace_identity_mismatch");
                }
                let response = ResponseFrame::WorkspaceSetup(SetupReceipt {
                    operation_id: request.operation_id.clone(),
                    request_sha256: request.request_sha256.clone(),
                    workspace_id: request.workspace_id.clone(),
                    commands: successful_commands(&request.commands),
                });
                remember_workspace_operation(
                    runtime,
                    request.operation_id,
                    request.request_sha256,
                    response.clone(),
                );
                Ok(response)
            }
            RequestFrame::RemoveWorkspace(request) => {
                require_credential(runtime, operation)?;
                if !request.has_valid_sha256() || request.operation_id.trim().is_empty() {
                    return refused(operation, "workspace_request_invalid");
                }
                if let Some(response) = replay_workspace_operation(
                    runtime,
                    &request.operation_id,
                    &request.request_sha256,
                    operation,
                )? {
                    return Ok(response);
                }
                let Some(workspace) = runtime.workspaces.get(&request.name) else {
                    return refused(operation, "workspace_not_found");
                };
                if workspace.workspace_id != request.workspace_id {
                    return refused(operation, "workspace_identity_mismatch");
                }
                runtime.workspaces.remove(&request.name);
                runtime
                    .worktrees
                    .retain(|(workspace, _), _| workspace != &request.name);
                let response = ResponseFrame::RemoveWorkspace(WorkspaceRemoveReceipt {
                    operation_id: request.operation_id.clone(),
                    request_sha256: request.request_sha256.clone(),
                    workspace_id: request.workspace_id.clone(),
                });
                remember_workspace_operation(
                    runtime,
                    request.operation_id,
                    request.request_sha256,
                    response.clone(),
                );
                Ok(response)
            }
            RequestFrame::AddWorktree(request) => {
                require_credential(runtime, operation)?;
                if !safe_name(&request.name)
                    || !runtime.workspaces.contains_key(&request.workspace)
                    || runtime
                        .worktrees
                        .contains_key(&(request.workspace.clone(), request.name.clone()))
                {
                    return refused(operation, "invalid_or_existing_worktree");
                }
                let receipt = WorktreeReceipt {
                    workspace: request.workspace.clone(),
                    name: request.name.clone(),
                    path: format!(
                        "/home/cma/{}/.worktrees/{}",
                        request.workspace, request.name
                    ),
                    branch: format!("cma/{}", request.name),
                    base_commit: request.base,
                };
                runtime
                    .worktrees
                    .insert((request.workspace, request.name), receipt.clone());
                Ok(ResponseFrame::AddWorktree(receipt))
            }
            RequestFrame::RemoveWorktree(reference) => {
                require_credential(runtime, operation)?;
                if runtime
                    .worktrees
                    .remove(&(reference.workspace, reference.name))
                    .is_none()
                {
                    return refused(operation, "worktree_not_found");
                }
                Ok(ResponseFrame::RemoveWorktree(Acknowledgement::default()))
            }
            RequestFrame::MaterializeAgentHome(plan) => {
                require_credential(runtime, operation)?;
                if plan.agent_id.0.is_empty() || runtime.homes.contains_key(&plan.agent_id) {
                    return refused(operation, "invalid_or_existing_agent_home");
                }
                runtime.homes.insert(plan.agent_id.clone(), plan);
                Ok(ResponseFrame::MaterializeAgentHome(
                    Acknowledgement::default(),
                ))
            }
            RequestFrame::CloseAgent(reference) => {
                require_credential(runtime, operation)?;
                let Some(agent) = runtime.agents.get_mut(&reference.agent_id) else {
                    return refused(operation, "agent_not_found");
                };
                agent.running = false;
                Ok(ResponseFrame::CloseAgent(Acknowledgement::default()))
            }
            RequestFrame::CaptureAgent(reference) => {
                require_credential(runtime, operation)?;
                let Some(agent) = runtime.agents.get(&reference.agent_id) else {
                    return refused(operation, "agent_not_found");
                };
                let archive = format!("conversation:{}", agent.conversation_id).into_bytes();
                let mut archive_sha256 = digest(&archive);
                if self.corrupt_capture_digest.swap(false, Ordering::Relaxed) {
                    archive_sha256 = "0".repeat(64);
                }
                Ok(ResponseFrame::CaptureAgent(ConversationSnapshot {
                    conversation_id: agent.conversation_id.clone(),
                    archive_sha256,
                    archive,
                }))
            }
            RequestFrame::Checkpoint(request) => {
                require_credential(runtime, operation)?;
                runtime.writer_fence_captures += 1;
                let marker = format!(
                    "{}:{}:{}",
                    handle.0, request.epoch, runtime.writer_fence_captures
                );
                Ok(ResponseFrame::Checkpoint(CheckpointReceipt {
                    epoch: request.epoch,
                    object_reference: format!("memory://checkpoint/{marker}"),
                    sha256: digest(marker.as_bytes()),
                    writers: WriterState::Resumed,
                }))
            }
            RequestFrame::Restore(request) => {
                require_credential(runtime, operation)?;
                runtime.restored_object = Some(request.object_reference);
                Ok(ResponseFrame::Restore(Acknowledgement::default()))
            }
            RequestFrame::Resources => Ok(ResponseFrame::Resources(ResourceReading {
                uptime_ms: 1,
                cpu_millis_limit: runtime
                    .request
                    .resource_limits
                    .get("cpu")
                    .and_then(|value| value.strip_suffix('m'))
                    .and_then(|value| value.parse().ok()),
                ..ResourceReading::default()
            })),
            RequestFrame::OpenAgent(_) => Err(AdapterError::Rejected(
                "open_agent must use the hosted conversation path".into(),
            )),
        }
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: AgentLaunch,
        resume_conversation: Option<String>,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<ConversationChannel> {
        self.requests
            .lock()
            .await
            .push((handle.clone(), RequestFrame::OpenAgent(launch.clone())));
        if let Some(failure) = self.queued_failure(WireOperation::OpenAgent) {
            return Err(AdapterError::Rejected(format!(
                "{}: {}",
                failure.code, failure.detail
            )));
        }

        let mut runtimes = self.runtimes.lock().await;
        let runtime = runtimes
            .get_mut(handle)
            .ok_or_else(|| AdapterError::Unavailable(format!("unknown runtime {}", handle.0)))?;
        require_credential(runtime, WireOperation::OpenAgent)?;
        if !runtime.homes.contains_key(&launch.agent_id)
            || !runtime
                .worktrees
                .values()
                .any(|worktree| worktree.path == launch.cwd)
            || runtime
                .agents
                .get(&launch.agent_id)
                .is_some_and(|agent| agent.running)
        {
            return Err(AdapterError::Rejected(
                "agent home, worktree, or slot is not ready".into(),
            ));
        }

        let conversation_id =
            resume_conversation.unwrap_or_else(|| format!("conversation-{}", launch.agent_id.0));
        runtime.agents.insert(
            launch.agent_id.clone(),
            MemoryAgent {
                launch,
                conversation_id: conversation_id.clone(),
                running: true,
            },
        );
        Ok(ConversationChannel {
            input: ConversationInputSender::spawn(1, |_| async { Ok(()) }),
            conversation_id: Some(conversation_id),
        })
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        if self.runtimes.lock().await.remove(handle).is_none() {
            return Err(AdapterError::Unavailable(format!(
                "unknown runtime {}",
                handle.0
            )));
        }
        Ok(())
    }
}

fn require_credential(runtime: &MemoryRuntime, operation: WireOperation) -> crate::Result<()> {
    if runtime.credential_delivered {
        Ok(())
    } else {
        Err(AdapterError::Rejected(format!(
            "{operation:?}: credential_not_delivered"
        )))
    }
}

fn replay_workspace_operation(
    runtime: &MemoryRuntime,
    operation_id: &str,
    request_sha256: &str,
    operation: WireOperation,
) -> crate::Result<Option<ResponseFrame>> {
    let Some((stored_digest, response)) = runtime.workspace_operations.get(operation_id) else {
        return Ok(None);
    };
    if stored_digest != request_sha256 {
        return refused(operation, "workspace_replay_mismatch");
    }
    Ok(Some(response.clone()))
}

fn remember_workspace_operation(
    runtime: &mut MemoryRuntime,
    operation_id: String,
    request_sha256: String,
    response: ResponseFrame,
) {
    runtime
        .workspace_operations
        .insert(operation_id, (request_sha256, response));
}

fn refused<T>(operation: WireOperation, code: &str) -> crate::Result<T> {
    Err(AdapterError::Rejected(format!("{operation:?}: {code}")))
}

fn successful_commands(commands: &[String]) -> Vec<CommandReceipt> {
    let empty = digest(&[]);
    commands
        .iter()
        .enumerate()
        .map(|(index, _)| CommandReceipt {
            index: index as u32,
            exit_code: 0,
            stdout_sha256: empty.clone(),
            stderr_sha256: empty.clone(),
        })
        .collect()
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn digest(bytes: &[u8]) -> String {
    let bytes = Sha256::digest(bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
