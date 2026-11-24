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
    ConversationChannel, ConversationSnapshot, ConversationUpdate, CredentialFrame,
    DesktopConnectionHandle, InstallReceipt, InstallRequest, ResourceReading,
    RestoreBootstrapReceipt, RestoreBootstrapRequest, RestoreRequest, RuntimeAdoptionRequest,
    RuntimeHandle, RuntimeObservation, RuntimeProvisionRequest, SandboxAdapter, SandboxSpec,
    SetupReceipt, WorkspaceReceipt, WorkspaceRemoveRequest, WorkspaceRequest,
    WorkspaceSetupRequest, WorktreeReceipt, WorktreeRef, WorktreeRequest, WriterState,
};

#[path = "adapter_agent.rs"]
mod agent;
#[path = "adapter_agent_resources.rs"]
mod agent_resources;
#[path = "adapter_attachment.rs"]
mod attachment;
#[path = "adapter_checkpoint.rs"]
mod checkpoint;
#[path = "adapter_context.rs"]
mod context;
#[path = "adapter_credential.rs"]
mod credential;
#[path = "adapter_exchange.rs"]
mod exchange;
#[path = "adapter_filesystem.rs"]
mod filesystem;
#[path = "adapter_finality.rs"]
mod finality;
#[path = "adapter_install.rs"]
mod install;
#[path = "adapter_nonexecuting.rs"]
mod nonexecuting;
#[path = "adapter_provider_tools.rs"]
mod provider_tools;
#[path = "adapter_runtime_identity.rs"]
mod runtime_identity;
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
    native_runtime_images: BTreeMap<String, crate::RuntimeImage>,
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
            native_runtime_images: BTreeMap::new(),
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

    /// Admit an additional deployment-owned image for native cold allocation.
    /// The caller must verify the installed image's contract before registration.
    pub fn with_native_runtime_image(mut self, image: crate::RuntimeImage) -> crate::Result<Self> {
        super::validate_runtime_image(&image.uri)?;
        crate::runtime_bootstrap::admit_pool_image(&image)?;
        self.native_runtime_images.insert(image.uri.clone(), image);
        Ok(self)
    }

    async fn select_native_pool_request(
        &self,
        request: &RuntimeProvisionRequest,
        home: &crate::HomeVolumeRequest,
    ) -> crate::Result<Option<crate::agent_sandbox::NativePoolBinding>> {
        let rendered = self.deployment.render_request(request)?;
        if let Some(image) = &request.native_canary_image {
            return self
                .backend
                .select_native_canary_pool(&rendered, image, home)
                .await
                .map(Some);
        }
        let image = match self.native_runtime_images.get(&rendered.image.uri) {
            Some(image) => image.clone(),
            // An image of the admitted release carries this build's contract.
            None => request
                .released
                .as_ref()
                .and_then(|released| released.image(&rendered.image.uri))
                .unwrap_or_else(|| self.deployment.image.clone()),
        };
        self.backend
            .select_native_pool(&rendered, &image, home)
            .await
    }

    pub(crate) async fn run_recovery_command(
        &self,
        handle: &RuntimeHandle,
        command: &super::RuntimeRecoveryCommand,
    ) -> crate::Result<Vec<u8>> {
        self.backend.run_recovery_command(handle, command).await
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

    pub(crate) async fn open_desktop(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<crate::DesktopConnectionHandle> {
        self.backend.open_desktop(handle).await
    }
}

#[path = "adapter_sandbox.rs"]
mod sandbox;

#[path = "adapter_retained_policy.rs"]
mod retained_policy;
