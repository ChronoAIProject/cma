use tokio::sync::mpsc;

use crate::provider::ProviderSandboxRequest;
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AgentHomePlan, AgentHomeRequest, AgentLaunch, AgentOpenRequest, ConversationChannel,
    ConversationUpdate, Result, RuntimeHandle, RuntimeObservation,
};

/// Kernel-specific settings used while opening its ACP conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedAgentOpenSettings {
    /// Trusted control-plane binding; never supplied by a provider renderer.
    pub context_refresh: Option<super::managed_context::HostedContextRefresh>,
    pub permission_mode: Option<String>,
    pub own_credential: bool,
    pub channel_tool: Option<super::HostedChannelToolBinding>,
    pub provider_tool: Option<super::HostedProviderToolBinding>,
    pub access_tool: Option<super::HostedAccessToolBinding>,
    /// Control-plane correlation for an already admitted turn; not launch data.
    pub recovered_prompt: Option<crate::PromptIdentity>,
}

/// Provider-specific rendering needed by the otherwise shared hosted adapter.
pub trait HostedAgentRuntime: Send + Sync {
    fn home_plan(&self, request: &AgentHomeRequest) -> Result<AgentHomePlan>;
    fn launch(&self, request: &AgentOpenRequest) -> Result<AgentLaunch>;

    fn open_settings(&self, request: &AgentOpenRequest) -> Result<HostedAgentOpenSettings> {
        Ok(HostedAgentOpenSettings {
            context_refresh: None,
            permission_mode: None,
            own_credential: request.options.own_credential,
            channel_tool: None,
            provider_tool: None,
            access_tool: None,
            recovered_prompt: None,
        })
    }
}

/// Provider lifecycle and supervisor transport used by a hosted adapter.
#[async_trait::async_trait]
pub trait HostedBackend: Send + Sync {
    async fn reserve_home_request(
        &self,
        _runtime: &ProviderSandboxRequest,
        home: &crate::HomeVolumeRequest,
    ) -> Result<crate::HomeVolumeRequest> {
        Ok(home.clone())
    }
    async fn verify_home_route(
        &self,
        _runtime: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<()> {
        Ok(())
    }
    fn verify_retained_home_route(
        &self,
        _handle: &RuntimeHandle,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<()> {
        Ok(())
    }
    fn native_pools(&self) -> Option<&dyn crate::agent_sandbox::NativePoolControl> {
        None
    }
    /// Resolve pool authority from the durable home before claim recovery or cleanup.
    fn native_pools_for_home(
        &self,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<Option<&dyn crate::agent_sandbox::NativePoolControl>> {
        Ok(self.native_pools())
    }
    async fn select_native_pool(
        &self,
        _request: &ProviderSandboxRequest,
        _image: &crate::RuntimeImage,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<Option<crate::agent_sandbox::NativePoolBinding>> {
        Ok(None)
    }
    async fn provision(&self, request: ProviderSandboxRequest) -> Result<RuntimeHandle>;
    /// Carry persisted home identity through provisioning without rediscovering it.
    /// Backends must still prove current resource authority before any effect.
    async fn provision_with_home(
        &self,
        request: ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<RuntimeHandle> {
        self.provision(request).await
    }
    async fn teardown_retained(&self, _handle: &RuntimeHandle) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "retained provider deletion is unavailable".into(),
        ))
    }
    async fn suspend(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
    ) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "provider suspension is unavailable".into(),
        ))
    }
    async fn verify_suspended(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
    ) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "provider suspension observation is unavailable".into(),
        ))
    }
    async fn resume(
        &self,
        _handle: &RuntimeHandle,
        _previous: &ProviderSandboxRequest,
        _next: &ProviderSandboxRequest,
    ) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "provider resumption is unavailable".into(),
        ))
    }
    /// Pass durable home authority to providers that can prove retained resources together.
    async fn resume_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<()> {
        self.resume(handle, previous, next).await
    }
    async fn upgrade_with_home(
        &self,
        _handle: &RuntimeHandle,
        _previous: &ProviderSandboxRequest,
        _next: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "native runtime upgrades are unavailable".into(),
        ))
    }
    async fn provision_receipted(
        &self,
        _request: ProviderSandboxRequest,
        _operation_id: &str,
        _request_sha256: &str,
    ) -> Result<crate::RuntimeProvisionReceipt> {
        Err(crate::AdapterError::Unsupported(
            "original hosted provider receipts are unavailable".into(),
        ))
    }
    async fn finalize_runtime(
        &self,
        _original: &crate::RuntimeProvisionReceipt,
        _operation_id: &str,
    ) -> Result<crate::RuntimeFinalityReceipt> {
        Err(crate::AdapterError::Unsupported(
            "hosted provider finality is unavailable".into(),
        ))
    }

    async fn runtime_termination(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<Option<crate::RuntimeTermination>> {
        Ok(None)
    }

    async fn inventory(&self) -> Result<Vec<RuntimeObservation>> {
        Err(crate::AdapterError::Unsupported(
            "hosted runtime inventory is not supported".into(),
        ))
    }

    async fn adopt(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
    ) -> Result<Option<RuntimeObservation>> {
        Err(crate::AdapterError::Unsupported(
            "hosted runtime adoption is not supported".into(),
        ))
    }

    /// Optional native optimization. None selects the unchanged full adoption path.
    /// The returned observation must include status; caller rechecks home afterward.
    async fn inspect_native_runtime(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<Option<crate::RuntimeInspection>> {
        Ok(None)
    }

    async fn reclaim(&self, _observation: &RuntimeObservation) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "hosted runtime reclamation is not supported".into(),
        ))
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> Result<ResponseFrame>;

    /// Revalidate a published checkpoint's writer hold without requiring export.
    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &crate::CheckpointHoldRequest,
    ) -> Result<ResponseFrame> {
        self.exchange(handle, RequestFrame::CheckpointForTeardown(request.clone()))
            .await
    }

    /// Open keeps the ACP channel out of the JSON control receipt while using
    /// the same launch frame that Layer 4 acknowledges.
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)>;

    async fn export_agent_history(
        &self,
        _handle: &RuntimeHandle,
        _agent: &crate::AgentSlot,
    ) -> Result<Vec<u8>> {
        Err(crate::AdapterError::Unsupported(
            "agent history export is not supported".into(),
        ))
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()>;
}
