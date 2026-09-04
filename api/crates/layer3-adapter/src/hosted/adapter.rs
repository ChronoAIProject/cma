use std::collections::BTreeMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, Mutex};

use super::reply::{
    expect_ack, failure_error, operation_name, response_operation, valid_sha256,
    validate_command_receipts,
};
use super::{HostedAgentRuntime, HostedBackend, HostedDeployment};
use crate::wire::{AgentRef, CredentialReceipt, RequestFrame, ResponseFrame, WorkspaceRef};
use crate::{
    AdapterCapabilities, AdapterError, AgentHomeRequest, AgentOpenRequest, AgentSlot,
    CheckpointReceipt, CheckpointRequest, ConversationChannel, ConversationSnapshot,
    ConversationUpdate, CredentialFrame, InstallReceipt, InstallRequest, ResourceReading,
    RestoreRequest, RuntimeHandle, SandboxAdapter, SandboxSpec, SetupReceipt, WorkspaceReceipt,
    WorkspaceRequest, WorkspaceSetupRequest, WorktreeReceipt, WorktreeRef, WorktreeRequest,
};

#[path = "adapter_install.rs"]
mod install;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Provisioned,
    Installing,
    Installed,
    DeliveringInitialCredential,
    RotatingCredential,
    Credentialed,
}

#[derive(Debug, Clone)]
struct RuntimeState {
    phase: Phase,
    own_credential: bool,
    provision: super::deployment::ProviderIdentity,
    install: Option<(String, String)>,
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

    async fn require_credential(&self, handle: &RuntimeHandle) -> crate::Result<RuntimeState> {
        let state = self.known_state(handle).await?;
        if !matches!(state.phase, Phase::Credentialed | Phase::RotatingCredential) {
            return Err(AdapterError::Rejected(format!(
                "runtime {} has not acknowledged its sandbox credential",
                handle.0
            )));
        }
        Ok(state)
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
        let state = self.require_credential(handle).await?;
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
                phase: Phase::Provisioned,
                own_credential: spec.own_credential,
                provision: identity,
                install: None,
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
        let rollback_phase = {
            let mut states = self.states.lock().await;
            let state = states.get_mut(handle).ok_or_else(|| {
                AdapterError::Unavailable(format!("unknown runtime {}", handle.0))
            })?;
            let rollback = match state.phase {
                Phase::Installed => Phase::Installed,
                Phase::Credentialed => Phase::Credentialed,
                _ => {
                    return Err(AdapterError::Rejected(
                        "credential delivery requires a successful install receipt".into(),
                    ))
                }
            };
            state.phase = if rollback == Phase::Credentialed {
                Phase::RotatingCredential
            } else {
                Phase::DeliveringInitialCredential
            };
            rollback
        };

        let operation_id = frame.operation_id.clone();
        let response = self.exchange(handle, RequestFrame::Credential(frame)).await;
        let mut states = self.states.lock().await;
        let state = states.get_mut(handle).ok_or_else(|| {
            AdapterError::Unavailable(format!(
                "runtime {} disappeared during credential delivery",
                handle.0
            ))
        })?;
        match response {
            Ok(ResponseFrame::Credential(CredentialReceipt {
                operation_id: returned,
                installed: true,
            })) if returned == operation_id => {
                state.phase = Phase::Credentialed;
                Ok(())
            }
            Ok(ResponseFrame::Credential(_)) => {
                state.phase = rollback_phase;
                Err(AdapterError::Rejected(
                    "runtime did not acknowledge the exact credential operation".into(),
                ))
            }
            Ok(_) => unreachable!("exchange verifies response operation"),
            Err(error) => {
                state.phase = rollback_phase;
                Err(error)
            }
        }
    }

    async fn prepare_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRequest,
    ) -> crate::Result<WorkspaceReceipt> {
        self.require_credential(handle).await?;
        match self
            .exchange(handle, RequestFrame::PrepareWorkspace(request.clone()))
            .await?
        {
            ResponseFrame::PrepareWorkspace(receipt) if receipt.name == request.name => Ok(receipt),
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
        self.require_credential(handle).await?;
        match self
            .exchange(handle, RequestFrame::WorkspaceSetup(request.clone()))
            .await?
        {
            ResponseFrame::WorkspaceSetup(receipt) => {
                validate_command_receipts(&receipt.commands, request.commands.len())?;
                Ok(receipt)
            }
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn remove_workspace(&self, handle: &RuntimeHandle, name: &str) -> crate::Result<()> {
        self.require_credential(handle).await?;
        expect_ack(
            self.exchange(
                handle,
                RequestFrame::RemoveWorkspace(WorkspaceRef { name: name.into() }),
            )
            .await?,
        )
    }

    async fn add_worktree(
        &self,
        handle: &RuntimeHandle,
        request: &WorktreeRequest,
    ) -> crate::Result<WorktreeReceipt> {
        self.require_credential(handle).await?;
        match self
            .exchange(handle, RequestFrame::AddWorktree(request.clone()))
            .await?
        {
            ResponseFrame::AddWorktree(receipt)
                if receipt.workspace == request.workspace && receipt.name == request.name =>
            {
                Ok(receipt)
            }
            ResponseFrame::AddWorktree(_) => Err(AdapterError::Rejected(
                "worktree receipt names a different worktree".into(),
            )),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn remove_worktree(
        &self,
        handle: &RuntimeHandle,
        request: &WorktreeRef,
    ) -> crate::Result<()> {
        self.require_credential(handle).await?;
        expect_ack(
            self.exchange(handle, RequestFrame::RemoveWorktree(request.clone()))
                .await?,
        )
    }

    async fn materialize_agent_home(
        &self,
        handle: &RuntimeHandle,
        request: &AgentHomeRequest,
    ) -> crate::Result<()> {
        self.validate_options(handle, &request.options).await?;
        let plan = self.runtime.home_plan(request)?;
        if plan.agent_id != request.options.slot {
            return Err(AdapterError::Rejected(
                "agent home plan changed the requested slot".into(),
            ));
        }
        expect_ack(
            self.exchange(handle, RequestFrame::MaterializeAgentHome(plan))
                .await?,
        )
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        request: &AgentOpenRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<ConversationChannel> {
        self.validate_options(handle, &request.options).await?;
        let launch = self.runtime.launch(request)?;
        if launch.agent_id != request.options.slot
            || launch.cwd != request.options.working_directory
            || launch.home != request.options.home_directory
        {
            return Err(AdapterError::Rejected(
                "agent launch changed its slot, working directory, or home".into(),
            ));
        }
        self.backend
            .open_agent(handle, launch, request.resume_conversation.clone(), updates)
            .await
    }

    async fn close_agent(&self, handle: &RuntimeHandle, agent: &AgentSlot) -> crate::Result<()> {
        self.require_credential(handle).await?;
        expect_ack(
            self.exchange(
                handle,
                RequestFrame::CloseAgent(AgentRef {
                    agent_id: agent.clone(),
                }),
            )
            .await?,
        )
    }

    async fn capture_agent(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> crate::Result<ConversationSnapshot> {
        self.require_credential(handle).await?;
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
        self.require_credential(handle).await?;
        match self
            .exchange(handle, RequestFrame::Checkpoint(request.clone()))
            .await?
        {
            ResponseFrame::Checkpoint(receipt)
                if receipt.epoch == request.epoch
                    && !receipt.object_reference.is_empty()
                    && valid_sha256(&receipt.sha256) =>
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
        self.require_credential(handle).await?;
        if request.object_reference.is_empty() || !valid_sha256(&request.sha256) {
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
