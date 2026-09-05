use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, Mutex};

use super::reply::{
    expect_ack, failure_error, operation_name, response_operation, valid_checkpoint_reference,
    valid_git_commit, valid_sha256, validate_command_receipts,
};
use super::{HostedAgentRuntime, HostedBackend, HostedDeployment};
use crate::wire::{AgentRef, CredentialReceipt, RequestFrame, ResponseFrame};
use crate::{
    AdapterCapabilities, AdapterError, AgentCloseRequest, AgentHomeRemoveRequest, AgentHomeRequest,
    AgentOpenRequest, AgentSlot, CheckpointReceipt, CheckpointRequest, ConversationChannel,
    ConversationSnapshot, ConversationUpdate, CredentialFrame, InstallReceipt, InstallRequest,
    ResourceReading, RestoreRequest, RuntimeHandle, SandboxAdapter, SandboxSpec, SetupReceipt,
    WorkspaceReceipt, WorkspaceRemoveRequest, WorkspaceRequest, WorkspaceSetupRequest,
    WorktreeReceipt, WorktreeRef, WorktreeRequest, WriterState,
};

#[path = "adapter_agent.rs"]
mod agent;
#[path = "adapter_install.rs"]
mod install;

#[derive(Debug, Clone)]
struct RuntimeState {
    own_credential: bool,
    home_directory: String,
    provision: super::deployment::ProviderIdentity,
}

/// Shared hosted implementation of the complete sandbox lifecycle.
pub struct HostedAdapter {
    backend: Arc<dyn HostedBackend>,
    runtime: Arc<dyn HostedAgentRuntime>,
    deployment: HostedDeployment,
    states: Mutex<BTreeMap<RuntimeHandle, RuntimeState>>,
}

impl HostedAdapter {
    pub fn new(
        backend: Arc<dyn HostedBackend>,
        runtime: Arc<dyn HostedAgentRuntime>,
        deployment: HostedDeployment,
    ) -> Self {
        Self {
            backend,
            runtime,
            deployment,
            states: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn deployment(&self) -> &HostedDeployment {
        &self.deployment
    }

    async fn known_state(&self, handle: &RuntimeHandle) -> crate::Result<RuntimeState> {
        self.states
            .lock()
            .await
            .get(handle)
            .cloned()
            .ok_or_else(|| AdapterError::Unavailable(format!("unknown runtime {}", handle.0)))
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        frame: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        let operation = frame.operation();
        let response = self.backend.exchange(handle, frame).await?;
        if let ResponseFrame::Failure(failure) = response {
            if failure.operation != operation {
                return Err(AdapterError::unexpected_reply(operation_name(operation)));
            }
            return Err(failure_error(failure));
        }
        if response_operation(&response) != operation {
            return Err(AdapterError::unexpected_reply(operation_name(operation)));
        }
        Ok(response)
    }

    async fn validate_options(
        &self,
        handle: &RuntimeHandle,
        options: &crate::RuntimeOptions,
    ) -> crate::Result<()> {
        let state = self.known_state(handle).await?;
        if options.own_credential != state.own_credential {
            return Err(AdapterError::Rejected(
                "agent credential mode differs from its sandbox".into(),
            ));
        }
        if options.slot.0.trim().is_empty()
            || !options.working_directory.starts_with('/')
            || !options.home_directory.starts_with('/')
        {
            return Err(AdapterError::Rejected(
                "agent slot, working directory, and home directory are required".into(),
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SandboxAdapter for HostedAdapter {
    fn capabilities(&self) -> AdapterCapabilities {
        self.deployment.capabilities()
    }

    async fn provision(&self, spec: &SandboxSpec) -> crate::Result<RuntimeHandle> {
        let request = self.deployment.render(spec)?;
        let identity = super::deployment::provider_request_identity(&request)?;
        let handle = self.backend.provision(request.clone()).await?;
        if handle.0.trim().is_empty() {
            return Err(AdapterError::Rejected(
                "provider returned an empty runtime handle".into(),
            ));
        }
        let mut states = self.states.lock().await;
        if let Some(state) = states.get(&handle) {
            if state.provision == identity {
                return Ok(handle);
            }
            return Err(AdapterError::Rejected(format!(
                "provider reused live runtime handle {}",
                handle.0
            )));
        }
        states.insert(
            handle.clone(),
            RuntimeState {
                own_credential: spec.own_credential,
                home_directory: spec.home_directory.clone(),
                provision: identity,
            },
        );
        Ok(handle)
    }

    async fn run_install(
        &self,
        handle: &RuntimeHandle,
        request: &InstallRequest,
    ) -> crate::Result<InstallReceipt> {
        self.run_install_request(handle, request).await
    }

    async fn deliver_credential(
        &self,
        handle: &RuntimeHandle,
        frame: CredentialFrame,
    ) -> crate::Result<()> {
        if frame.operation_id.trim().is_empty()
            || frame.value.expose().is_empty()
            || frame.custody_generation == Some(0)
        {
            return Err(AdapterError::Rejected(
                "credential operation id, value, and non-zero generation are required".into(),
            ));
        }
        self.known_state(handle).await?;
        let operation_id = frame.operation_id.clone();
        match self.exchange(handle, RequestFrame::Credential(frame)).await {
            Ok(ResponseFrame::Credential(CredentialReceipt {
                operation_id: returned,
                installed: true,
            })) if returned == operation_id => Ok(()),
            Ok(ResponseFrame::Credential(_)) => Err(AdapterError::Rejected(
                "runtime did not acknowledge the exact credential operation".into(),
            )),
            Ok(_) => unreachable!("exchange verifies response operation"),
            Err(error) => Err(error),
        }
    }

    async fn prepare_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRequest,
    ) -> crate::Result<WorkspaceReceipt> {
        let state = self.known_state(handle).await?;
        validate_workspace_operation(
            &request.operation_id,
            &request.request_sha256,
            request.has_valid_sha256(),
        )?;
        match self
            .exchange(handle, RequestFrame::PrepareWorkspace(request.clone()))
            .await?
        {
            ResponseFrame::PrepareWorkspace(receipt)
                if receipt.name == request.name
                    && receipt.workspace_id == request.workspace_id
                    && receipt.operation_id == request.operation_id
                    && receipt.request_sha256 == request.request_sha256
                    && Path::new(&receipt.path)
                        == Path::new(&state.home_directory).join(&request.name)
                    && valid_git_commit(&receipt.resolved_commit) =>
            {
                Ok(receipt)
            }
            ResponseFrame::PrepareWorkspace(_) => Err(AdapterError::Rejected(
                "workspace receipt names a different workspace".into(),
            )),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn run_workspace_setup(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceSetupRequest,
    ) -> crate::Result<SetupReceipt> {
        self.known_state(handle).await?;
        validate_workspace_operation(
            &request.operation_id,
            &request.request_sha256,
            request.has_valid_sha256(),
        )?;
        if request.timeout_seconds == 0 || request.output_limit_bytes == 0 {
            return Err(AdapterError::Rejected(
                "workspace setup command bounds are invalid".into(),
            ));
        }
        match self
            .exchange(handle, RequestFrame::WorkspaceSetup(request.clone()))
            .await?
        {
            ResponseFrame::WorkspaceSetup(receipt) => {
                if receipt.operation_id != request.operation_id
                    || receipt.request_sha256 != request.request_sha256
                    || receipt.workspace_id != request.workspace_id
                {
                    return Err(AdapterError::Rejected(
                        "runtime returned a receipt for another workspace setup operation".into(),
                    ));
                }
                validate_command_receipts(&receipt.commands, request.commands.len())?;
                Ok(receipt)
            }
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn remove_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRemoveRequest,
    ) -> crate::Result<()> {
        self.known_state(handle).await?;
        validate_workspace_operation(
            &request.operation_id,
            &request.request_sha256,
            request.has_valid_sha256(),
        )?;
        match self
            .exchange(handle, RequestFrame::RemoveWorkspace(request.clone()))
            .await?
        {
            ResponseFrame::RemoveWorkspace(receipt)
                if receipt.operation_id == request.operation_id
                    && receipt.request_sha256 == request.request_sha256
                    && receipt.workspace_id == request.workspace_id =>
            {
                Ok(())
            }
            ResponseFrame::RemoveWorkspace(_) => Err(AdapterError::Rejected(
                "runtime returned a receipt for another workspace removal operation".into(),
            )),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn add_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRequest,
    ) -> crate::Result<WorktreeReceipt> {
        agent::add_worktree(self, handle, authority_fence, request).await
    }

    async fn remove_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRef,
    ) -> crate::Result<()> {
        agent::remove_worktree(self, handle, authority_fence, request).await
    }

    async fn materialize_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRequest,
    ) -> crate::Result<()> {
        agent::materialize_home(self, handle, authority_fence, request).await
    }

    async fn remove_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRemoveRequest,
    ) -> crate::Result<()> {
        agent::remove_home(self, handle, authority_fence, request).await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<ConversationChannel> {
        agent::open_agent(self, handle, authority_fence, request, updates).await
    }

    async fn close_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentCloseRequest,
    ) -> crate::Result<()> {
        agent::close_agent(self, handle, authority_fence, request).await
    }

    async fn capture_agent(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> crate::Result<ConversationSnapshot> {
        self.known_state(handle).await?;
        match self
            .exchange(
                handle,
                RequestFrame::CaptureAgent(AgentRef {
                    agent_id: agent.clone(),
                }),
            )
            .await?
        {
            ResponseFrame::CaptureAgent(snapshot)
                if !snapshot.conversation_id.is_empty()
                    && valid_sha256(&snapshot.archive_sha256)
                    && format!("{:x}", Sha256::digest(&snapshot.archive))
                        == snapshot.archive_sha256 =>
            {
                Ok(snapshot)
            }
            ResponseFrame::CaptureAgent(_) => Err(AdapterError::Rejected(
                "runtime returned an invalid conversation snapshot".into(),
            )),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn checkpoint(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointRequest,
    ) -> crate::Result<CheckpointReceipt> {
        self.known_state(handle).await?;
        match self
            .exchange(handle, RequestFrame::Checkpoint(request.clone()))
            .await?
        {
            ResponseFrame::Checkpoint(receipt)
                if receipt.epoch == request.epoch
                    && valid_checkpoint_reference(&receipt.object_reference, &receipt.sha256)
                    && receipt.writers == WriterState::Resumed =>
            {
                Ok(receipt)
            }
            ResponseFrame::Checkpoint(_) => Err(AdapterError::Rejected(
                "runtime returned an invalid checkpoint receipt".into(),
            )),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn restore(&self, handle: &RuntimeHandle, request: &RestoreRequest) -> crate::Result<()> {
        self.known_state(handle).await?;
        if !valid_checkpoint_reference(&request.object_reference, &request.sha256) {
            return Err(AdapterError::Rejected(
                "restore object reference or digest is invalid".into(),
            ));
        }
        expect_ack(
            self.exchange(handle, RequestFrame::Restore(request.clone()))
                .await?,
        )
    }

    async fn resources(&self, handle: &RuntimeHandle) -> crate::Result<ResourceReading> {
        self.known_state(handle).await?;
        match self.exchange(handle, RequestFrame::Resources).await? {
            ResponseFrame::Resources(reading) => Ok(reading),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        self.known_state(handle).await?;
        self.backend.teardown(handle).await?;
        self.states.lock().await.remove(handle);
        Ok(())
    }
}

fn validate_workspace_operation(
    operation_id: &str,
    request_sha256: &str,
    digest_matches: bool,
) -> crate::Result<()> {
    if operation_id.trim().is_empty()
        || operation_id.len() > 256
        || operation_id.chars().any(char::is_control)
        || !valid_sha256(request_sha256)
        || !digest_matches
    {
        return Err(AdapterError::Rejected(
            "workspace operation id or request digest is invalid".into(),
        ));
    }
    Ok(())
}
