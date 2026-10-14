use std::collections::BTreeMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, Mutex};

use super::reply::{
    expect_ack, failure_error, operation_name, response_operation, valid_checkpoint_reference,
    valid_sha256,
};
use super::{HostedAgentRuntime, HostedBackend, HostedDeployment};
use crate::wire::{AgentRef, CredentialReceipt, RequestFrame, ResponseFrame};
use crate::{
    AdapterCapabilities, AdapterError, AgentCloseRequest, AgentHomeRemoveRequest, AgentHomeRequest,
    AgentOpenRequest, AgentSlot, CheckpointHoldReceipt, CheckpointHoldRequest, CheckpointReceipt,
    CheckpointRequest, CheckpointWriterResumeReceipt, CheckpointWriterResumeRequest,
    ConversationChannel, ConversationSnapshot, ConversationUpdate, CredentialFrame, InstallReceipt,
    InstallRequest, ResourceReading, RestoreBootstrapReceipt, RestoreBootstrapRequest,
    RestoreRequest, RuntimeAdoptionRequest, RuntimeHandle, RuntimeObservation,
    RuntimeProvisionRequest, SandboxAdapter, SandboxSpec, SetupReceipt, WorkspaceReceipt,
    WorkspaceRemoveRequest, WorkspaceRequest, WorkspaceSetupRequest, WorktreeReceipt, WorktreeRef,
    WorktreeRequest, WriterState,
};

#[path = "adapter_agent.rs"]
mod agent;
#[path = "adapter_checkpoint.rs"]
mod checkpoint;
#[path = "adapter_context.rs"]
mod context;
#[path = "adapter_exchange.rs"]
mod exchange;
#[path = "adapter_install.rs"]
mod install;
#[path = "adapter_provider_tools.rs"]
mod provider_tools;
#[path = "adapter_runtime.rs"]
mod runtime_lifecycle;
#[path = "adapter_workspace.rs"]
mod workspace;
#[path = "adapter_writer_hold.rs"]
mod writer_hold;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeState {
    own_credential: bool,
    home_directory: String,
    provision: super::deployment::ProviderIdentity,
    home_volume: Option<Box<crate::HomeVolumeAttachment>>,
}

/// Shared hosted implementation of the complete sandbox lifecycle.
pub struct HostedAdapter {
    backend: Arc<dyn HostedBackend>,
    runtime: Arc<dyn HostedAgentRuntime>,
    deployment: HostedDeployment,
    home_volumes: Option<Arc<dyn crate::HomeVolumeStore>>,
    channel_tools: Option<Arc<dyn super::HostedChannelToolSource>>,
    access_tools: Option<Arc<dyn super::HostedAccessToolSource>>,
    provider_tools: Option<Arc<dyn super::HostedProviderToolSource>>,
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
            home_volumes: None,
            channel_tools: None,
            access_tools: None,
            provider_tools: None,
            states: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn deployment(&self) -> &HostedDeployment {
        &self.deployment
    }

    pub fn with_home_volumes(mut self, store: Arc<dyn crate::HomeVolumeStore>) -> Self {
        self.home_volumes = Some(store);
        self
    }

    pub fn with_access_tools(mut self, source: Arc<dyn super::HostedAccessToolSource>) -> Self {
        self.access_tools = Some(source);
        self
    }

    pub fn with_channel_tools(mut self, source: Arc<dyn super::HostedChannelToolSource>) -> Self {
        self.channel_tools = Some(source);
        self
    }

    async fn known_state(&self, handle: &RuntimeHandle) -> crate::Result<RuntimeState> {
        self.states
            .lock()
            .await
            .get(handle)
            .cloned()
            .ok_or_else(|| AdapterError::Unavailable(format!("unknown runtime {}", handle.0)))
    }
}

#[path = "adapter_sandbox.rs"]
mod sandbox;
