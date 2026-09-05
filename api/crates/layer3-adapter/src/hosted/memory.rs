use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{mpsc, Mutex};

use super::deployment::provider_request_identity;
use super::HostedBackend;
use crate::provider::ProviderSandboxRequest;
use crate::wire::{
    Acknowledgement, AgentOpenReceipt, CredentialReceipt, FencedReceipt, FencedRequest,
    RequestFrame, ResponseFrame, WireFailure, WireOperation,
};
use crate::{
    AdapterError, AgentHomePlan, AgentLaunch, AgentSlot, CheckpointReceipt, CommandReceipt,
    ConversationChannel, ConversationInputSender, ConversationSnapshot, ConversationUpdate,
    InstallReceipt, ResourceReading, RuntimeHandle, WorkspaceReceipt, WorktreeReceipt, WriterState,
};

#[path = "memory_state.rs"]
mod state;
use state::{
    digest, fingerprint, refused, remember_operation, replay_operation, require_credential,
    successful_commands, valid_sha256,
};
#[path = "memory_authority.rs"]
mod authority;
#[path = "memory_mutations.rs"]
mod mutations;

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
    connection_fence: u64,
    updates: mpsc::Sender<ConversationUpdate>,
}

#[derive(Debug, Clone)]
struct MemoryRuntime {
    request: ProviderSandboxRequest,
    install: Option<((String, String), InstallReceipt)>,
    credential_delivered: bool,
    workspaces: BTreeMap<String, WorkspaceReceipt>,
    operations: BTreeMap<String, MemoryOperation>,
    authorities: BTreeMap<authority::ResourceKey, authority::Authority>,
    worktrees: BTreeMap<(String, String), WorktreeReceipt>,
    homes: BTreeMap<AgentSlot, AgentHomePlan>,
    agents: BTreeMap<AgentSlot, MemoryAgent>,
    writer_fence_captures: u64,
    restored_object: Option<String>,
}

#[derive(Debug, Clone)]
struct MemoryOperation {
    request_sha256: String,
    payload_sha256: String,
    response: ResponseFrame,
}

/// Deterministic hosted backend with no filesystem or network effects.
#[derive(Clone, Default)]
pub struct InMemoryHostedBackend {
    next_handle: Arc<AtomicU64>,
    runtimes: Arc<Mutex<BTreeMap<RuntimeHandle, MemoryRuntime>>>,
    requests: Arc<Mutex<Vec<(RuntimeHandle, RequestFrame)>>>,
    failures: Arc<StdMutex<VecDeque<WireFailure>>>,
    lost_responses: Arc<StdMutex<VecDeque<WireOperation>>>,
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

    /// Emit one conversation update from an inspectable runtime.
    pub async fn send_agent_update(
        &self,
        handle: &RuntimeHandle,
        agent_id: &AgentSlot,
        update: ConversationUpdate,
    ) -> crate::Result<()> {
        let updates = self
            .runtimes
            .lock()
            .await
            .get(handle)
            .and_then(|runtime| runtime.agents.get(agent_id))
            .filter(|agent| agent.running)
            .map(|agent| agent.updates.clone())
            .ok_or_else(|| AdapterError::Unavailable("agent update channel is not live".into()))?;
        updates
            .send(update)
            .await
            .map_err(|_| AdapterError::Unavailable("agent update receiver ended".into()))
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

    /// Apply one operation but report a lost response to exercise durable replay.
    pub fn lose_next_response(&self, operation: WireOperation) {
        self.lost_responses
            .lock()
            .expect("in-memory lost-response queue lock")
            .push_back(operation);
    }

    /// Make the next successful install reply carry an invalid receipt.
    pub fn corrupt_next_install_receipt(&self) {
        self.corrupt_install_receipt.store(true, Ordering::Relaxed);
    }

    /// Make the next captured archive disagree with its advertised digest.
    pub fn corrupt_next_capture_digest(&self) {
        self.corrupt_capture_digest.store(true, Ordering::Relaxed);
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
                operations: BTreeMap::new(),
                authorities: BTreeMap::new(),
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
        let response = match request {
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
                mutations::prepare_workspace(runtime, request)
            }
            RequestFrame::WorkspaceSetup(request) => {
                require_credential(runtime, operation)?;
                mutations::setup_workspace(runtime, request)
            }
            RequestFrame::RemoveWorkspace(request) => {
                require_credential(runtime, operation)?;
                mutations::remove_workspace(runtime, request)
            }
            RequestFrame::AddWorktree(request) => {
                require_credential(runtime, operation)?;
                mutations::add_worktree(runtime, request)
            }
            RequestFrame::RemoveWorktree(request) => {
                require_credential(runtime, operation)?;
                mutations::remove_worktree(runtime, request)
            }
            RequestFrame::MaterializeAgentHome(plan) => {
                require_credential(runtime, operation)?;
                mutations::materialize_home(runtime, plan)
            }
            RequestFrame::RemoveAgentHome(request) => {
                require_credential(runtime, operation)?;
                mutations::remove_home(runtime, request)
            }
            RequestFrame::CloseAgent(request) => {
                require_credential(runtime, operation)?;
                mutations::close_agent(runtime, request)
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
                let sha256 = digest(marker.as_bytes());
                Ok(ResponseFrame::Checkpoint(CheckpointReceipt {
                    epoch: request.epoch,
                    object_reference: format!("home-{sha256}.cma"),
                    sha256,
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
        }?;
        if self.take_lost_response(operation) {
            return Err(AdapterError::Unavailable(format!(
                "{operation:?}: response lost after effect"
            )));
        }
        Ok(response)
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
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
        let authority_fence = launch.authority_fence;
        let launch = launch.request;
        let expected = format!("agent:{}:open:v1", launch.agent_id.as_str());
        if launch.operation_id != expected
            || !valid_sha256(&launch.request_sha256)
            || authority_fence == 0
        {
            return refused(WireOperation::OpenAgent, "operation_request_invalid");
        }
        let payload = fingerprint(&(launch.clone(), resume_conversation.clone()))?;
        authority::admit(
            runtime,
            authority::ResourceKey::AgentProcess(launch.agent_id.clone()),
            authority::Disposition::Present,
            authority_fence,
            &launch.operation_id,
            &launch.request_sha256,
            &launch.payload_sha256(),
            WireOperation::OpenAgent,
        )?;
        if let Some(response) = replay_operation(
            runtime,
            &launch.operation_id,
            &launch.request_sha256,
            &payload,
            WireOperation::OpenAgent,
        )? {
            let ResponseFrame::OpenAgent(mut receipt) = response else {
                return refused(WireOperation::OpenAgent, "operation_replay_mismatch");
            };
            receipt.authority_fence = authority_fence;
            let live = runtime
                .agents
                .get(&launch.agent_id)
                .is_some_and(|agent| agent.running && agent.launch == launch);
            if !live {
                return refused(WireOperation::OpenAgent, "agent_process_not_live");
            }
            if let Some(agent) = runtime.agents.get_mut(&launch.agent_id) {
                if authority_fence > agent.connection_fence {
                    agent.connection_fence = authority_fence;
                    agent.updates = updates;
                }
            }
            let result = (channel(receipt.receipt.conversation_id.clone()), receipt);
            if self.take_lost_response(WireOperation::OpenAgent) {
                return Err(AdapterError::Unavailable(
                    "OpenAgent: response lost after effect".into(),
                ));
            }
            return Ok(result);
        }
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
        let receipt = AgentOpenReceipt {
            operation_id: launch.operation_id.clone(),
            request_sha256: launch.request_sha256.clone(),
            agent_id: launch.agent_id.clone(),
            cwd: launch.cwd.clone(),
            home: launch.home.clone(),
            conversation_id: Some(conversation_id.clone()),
        };
        runtime.agents.insert(
            launch.agent_id.clone(),
            MemoryAgent {
                launch,
                conversation_id: conversation_id.clone(),
                running: true,
                connection_fence: authority_fence,
                updates,
            },
        );
        remember_operation(
            runtime,
            receipt.operation_id.clone(),
            receipt.request_sha256.clone(),
            payload,
            ResponseFrame::OpenAgent(FencedReceipt::new(authority_fence, receipt.clone())),
        );
        let result = (channel(Some(conversation_id)), receipt);
        if self.take_lost_response(WireOperation::OpenAgent) {
            return Err(AdapterError::Unavailable(
                "OpenAgent: response lost after effect".into(),
            ));
        }
        Ok((result.0, FencedReceipt::new(authority_fence, result.1)))
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

fn channel(conversation_id: Option<String>) -> ConversationChannel {
    ConversationChannel {
        input: ConversationInputSender::spawn(1, |_| async { Ok(()) }),
        conversation_id,
    }
}
