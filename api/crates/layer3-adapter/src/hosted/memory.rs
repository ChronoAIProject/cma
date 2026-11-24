use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{mpsc, Mutex};

use super::deployment::provider_request_identity;
use super::{HostedAgentOpenSettings, HostedBackend};
use crate::provider::ProviderSandboxRequest;
use crate::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame, WireFailure,
    WireOperation,
};
use crate::{
    AdapterError, AgentHomePlan, AgentLaunch, AgentSlot, CheckpointHoldReceipt,
    CheckpointHoldRequest, ConversationChannel, ConversationSnapshot, ConversationUpdate,
    InstallReceipt, RestoreBootstrapReceipt, RuntimeHandle, RuntimeObservation, WorkspaceReceipt,
    WorktreeReceipt,
};

#[path = "memory_state.rs"]
mod state;
pub use state::InMemoryRuntimeSnapshot;
use state::{
    channel, digest, fingerprint, memory_observation, refused, remember_operation,
    replay_operation, require_credential,
};
#[path = "memory_authority.rs"]
mod authority;
#[path = "memory_bootstrap.rs"]
mod bootstrap;
#[path = "memory_checkpoint.rs"]
mod checkpoint;
#[path = "memory_home.rs"]
mod home;
#[path = "memory_mutations.rs"]
mod mutations;
#[path = "memory_probes.rs"]
mod probes;
#[path = "memory_process.rs"]
mod process;
#[path = "memory_status.rs"]
mod status;
#[path = "memory_workspace.rs"]
mod workspace;

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
    boot_id: String,
    install: Option<((String, String), InstallReceipt)>,
    restore_bootstrap: Option<RestoreBootstrapReceipt>,
    credential_delivered: bool,
    workspaces: BTreeMap<String, WorkspaceReceipt>,
    operations: BTreeMap<String, MemoryOperation>,
    authorities: BTreeMap<authority::ResourceKey, authority::Authority>,
    worktrees: BTreeMap<(String, String), WorktreeReceipt>,
    homes: BTreeMap<AgentSlot, AgentHomePlan>,
    agents: BTreeMap<AgentSlot, MemoryAgent>,
    writer_fence_captures: u64,
    held_checkpoint: Option<(CheckpointHoldRequest, CheckpointHoldReceipt)>,
    resumed_checkpoint_tokens: BTreeSet<String>,
    restored_object: Option<String>,
    restored_conversations: BTreeMap<AgentSlot, String>,
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
    retained_homes: Arc<Mutex<BTreeMap<String, MemoryRuntime>>>,
    requests: Arc<Mutex<Vec<(RuntimeHandle, RequestFrame)>>>,
    failures: Arc<StdMutex<VecDeque<WireFailure>>>,
    lost_responses: Arc<StdMutex<VecDeque<WireOperation>>>,
    corrupt_install_receipt: Arc<AtomicBool>,
    corrupt_capture_digest: Arc<AtomicBool>,
    inventory_unavailable: Arc<AtomicBool>,
    archives: Arc<StdMutex<BTreeMap<String, checkpoint::HomeArchive>>>,
}

#[async_trait::async_trait]
impl HostedBackend for InMemoryHostedBackend {
    fn startup_binding_headroom(&self, _handles: &[RuntimeHandle]) -> Option<usize> {
        // This backend retains all runtime entries; adoption has no eviction cache.
        Some(usize::MAX)
    }
    async fn provision(&self, request: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        let identity = provider_request_identity(&request)?;
        let mut runtimes = self.runtimes.lock().await;
        if let Some((handle, runtime)) = runtimes.iter().find(|(_, runtime)| {
            provider_request_identity(&runtime.request).is_ok_and(|existing| {
                existing.sandbox_id == identity.sandbox_id
                    && existing.generation == identity.generation
            })
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
        let mut runtime = MemoryRuntime {
            request,
            boot_id: format!("{id:032x}"),
            install: None,
            restore_bootstrap: None,
            credential_delivered: false,
            workspaces: BTreeMap::new(),
            operations: BTreeMap::new(),
            authorities: BTreeMap::new(),
            worktrees: BTreeMap::new(),
            homes: BTreeMap::new(),
            agents: BTreeMap::new(),
            writer_fence_captures: 0,
            held_checkpoint: None,
            resumed_checkpoint_tokens: BTreeSet::new(),
            restored_object: None,
            restored_conversations: BTreeMap::new(),
        };
        let retained = self.retained_homes.lock().await;
        home::attach(&mut runtime, &runtimes, &retained)?;
        runtimes.insert(handle.clone(), runtime);
        Ok(handle)
    }
    async fn inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        if self.inventory_unavailable.load(Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "provider inventory is unavailable".into(),
            ));
        }
        self.runtimes
            .lock()
            .await
            .iter()
            .map(|(handle, runtime)| memory_observation(handle, &runtime.request))
            .collect()
    }

    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> crate::Result<Option<RuntimeObservation>> {
        let expected = provider_request_identity(request)?;
        let runtimes = self.runtimes.lock().await;
        let Some(runtime) = runtimes.get(handle) else {
            return Ok(None);
        };
        if provider_request_identity(&runtime.request)? != expected {
            return Err(AdapterError::Rejected(
                "provider runtime belongs to another exact identity".into(),
            ));
        }
        memory_observation(handle, &runtime.request).map(Some)
    }

    async fn reclaim(&self, observation: &RuntimeObservation) -> crate::Result<()> {
        let mut runtimes = self.runtimes.lock().await;
        let removed = state::reclaim(&mut runtimes, observation)?;
        if let Some(runtime) = removed {
            let mut retained = self.retained_homes.lock().await;
            home::retain(runtime, &mut retained)?;
        }
        Ok(())
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        let operation = request.operation();
        if request.has_unexpected_restart_runtime() {
            return refused(operation, "agent_process_restart_runtime_unexpected");
        }
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
        if runtime.held_checkpoint.is_some()
            && !matches!(
                &request,
                RequestFrame::CheckpointForTeardown(_)
                    | RequestFrame::ResumeCheckpointWriters(_)
                    | RequestFrame::CaptureAgent(_)
                    | RequestFrame::Resources
                    | RequestFrame::RuntimeStatus(_)
                    | RequestFrame::ToolProxyStatus(_)
            )
        {
            return refused(operation, "checkpoint_writers_held");
        }
        let response = match request {
            RequestFrame::Browser(_) => return refused(operation, "browser_unsupported"),
            RequestFrame::BrowserControl(_) => return refused(operation, "browser_unsupported"),
            RequestFrame::UploadAgentAttachment(_) => {
                return refused(operation, "attachment_upload_unsupported")
            }
            RequestFrame::ListAgentFiles(_)
            | RequestFrame::ReadAgentFile(_)
            | RequestFrame::WriteAgentFile(_)
            | RequestFrame::DeleteAgentFile(_) => {
                return refused(operation, "filesystem_unsupported")
            }
            RequestFrame::CacheSkillArchive(archive) => {
                require_credential(runtime, operation)?;
                archive.validate()?;
                Ok(ResponseFrame::CacheSkillArchive {
                    sha256: archive.sha256,
                })
            }
            RequestFrame::Install(request) => {
                bootstrap::install(runtime, &self.corrupt_install_receipt, request, operation)
            }
            RequestFrame::BootstrapRestore(request) => {
                bootstrap::restore(runtime, request, operation)
            }
            RequestFrame::Credential(frame) => bootstrap::credential(runtime, frame, operation),
            RequestFrame::CredentialForReceiver { .. } => {
                refused(operation, "credential_github_route_unavailable")
            }
            RequestFrame::PrepareWorkspace(request) => {
                require_credential(runtime, operation)?;
                workspace::prepare(runtime, request)
            }
            RequestFrame::WorkspaceSetup(request) => {
                require_credential(runtime, operation)?;
                workspace::setup(runtime, request)
            }
            RequestFrame::RemoveWorkspace(request) => {
                require_credential(runtime, operation)?;
                workspace::remove(runtime, request)
            }
            RequestFrame::PrepareAgentResources(_) => {
                // The in-memory backend does not advertise this optional runtime operation.
                refused(operation, "agent_resource_preparation_unsupported")
            }
            RequestFrame::AddWorktree(request) => {
                require_credential(runtime, operation)?;
                mutations::add_worktree(runtime, request)
            }
            RequestFrame::RemoveWorktree(request) => {
                require_credential(runtime, operation)?;
                mutations::remove_worktree(runtime, request)
            }
            RequestFrame::InstallAgentSkill(request) => {
                require_credential(runtime, operation)?;
                mutations::install_skill(runtime, request)
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
                process::close_agent(runtime, request)
            }
            RequestFrame::FenceAgentRestart(_) => {
                return refused(operation, "agent_restart_fence_unsupported");
            }
            RequestFrame::ProveAgentProcessClosed(_) => {
                return refused(operation, "agent_process_close_proof_unsupported");
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
                checkpoint::capture(runtime, &self.archives, request, operation)
            }
            RequestFrame::CheckpointForTeardown(request) => {
                checkpoint::capture_for_teardown(runtime, &self.archives, request, operation)
            }
            RequestFrame::ResumeCheckpointWriters(request) => {
                checkpoint::resume_writers(runtime, request, operation)
            }
            RequestFrame::Restore(request) => {
                checkpoint::restore(runtime, &self.archives, request, operation)
            }
            RequestFrame::Resources => checkpoint::resources(runtime),
            RequestFrame::RuntimeStatus(request) => status::inspect(runtime, request),
            RequestFrame::ToolProxyStatus(_) => Err(AdapterError::Unsupported(
                "this runtime has no credential tool proxy".into(),
            )),
            RequestFrame::RecoverWorkspacePrepare(_)
            | RequestFrame::WorkspaceRemovalSupport(_)
            | RequestFrame::ReconcileWorkspaceRemoval(_) => Err(AdapterError::Unsupported(
                "guarded Workspace removal is unavailable in this runtime".into(),
            )),
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
        _settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.requests
            .lock()
            .await
            .push((handle.clone(), RequestFrame::OpenAgent(launch.clone())));
        if let Some(failure) = self.queued_failure(WireOperation::OpenAgent) {
            if super::reply::ledger_hold(&failure) {
                return Err(super::reply::failure_error(failure));
            }
            return Err(AdapterError::Rejected(format!(
                "{}: {}",
                failure.code, failure.detail
            )));
        }

        let mut runtimes = self.runtimes.lock().await;
        let runtime = runtimes
            .get_mut(handle)
            .ok_or_else(|| AdapterError::Unavailable(format!("unknown runtime {}", handle.0)))?;
        if runtime.held_checkpoint.is_some() {
            return refused(WireOperation::OpenAgent, "checkpoint_writers_held");
        }
        require_credential(runtime, WireOperation::OpenAgent)?;
        let incarnation = process::authorize_open(runtime, &launch)?;
        let authority_fence = launch.authority_fence;
        let launch = launch.request;
        let payload = fingerprint(&launch)?;
        authority::admit(
            runtime,
            authority::ResourceKey::AgentProcess(launch.agent_id.clone(), incarnation),
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

        if runtime.restored_object.is_some()
            && resume_conversation.as_ref() != runtime.restored_conversations.get(&launch.agent_id)
        {
            return refused(WireOperation::OpenAgent, "restored_conversation_mismatch");
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
        let mut runtimes = self.runtimes.lock().await;
        let runtime = runtimes
            .remove(handle)
            .ok_or_else(|| AdapterError::Unavailable(format!("unknown runtime {}", handle.0)))?;
        let mut retained = self.retained_homes.lock().await;
        home::retain(runtime, &mut retained)?;
        Ok(())
    }
}
