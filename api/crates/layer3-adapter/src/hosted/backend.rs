use tokio::sync::mpsc;

use super::RuntimeRecoveryCommand;
use crate::provider::ProviderSandboxRequest;
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AgentHomePlan, AgentHomeRequest, AgentLaunch, AgentOpenRequest, ConversationChannel,
    ConversationUpdate, Result, RuntimeHandle, RuntimeObservation,
};

/// Kernel-specific settings used while opening its ACP conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedAgentOpenSettings {
    /// Trusted transport policy; the provider renderer cannot override it.
    pub coordinated_ingress: bool,
    pub retained_json: bool,
    pub bounded_raw_output: bool,
    /// Transport-owned compact permission retention policy.
    pub bounded_permission_state: bool,
    /// Transport policy, overwritten from trusted composition before attachment.
    pub output_json_budget: bool,
    /// Transport-owned policy for immutable retained domain payloads.
    pub shared_runtime_output: bool,
    /// Exact provider-proved initialize error for an already initialized bridge.
    /// This permits reattachment only; it never supplies conversation/tool authority.
    pub already_initialized_error: Option<serde_json::Value>,
    /// Provider-proved read-only evidence of an exact completed conversation open.
    pub open_completion: Option<super::HostedOpenCompletionProtocol>,
    /// Trusted control-plane binding; never supplied by a provider renderer.
    pub context_refresh: Option<super::managed_context::HostedContextRefresh>,
    pub permission_mode: Option<String>,
    pub own_credential: bool,
    pub channel_tool: Option<super::HostedChannelToolBinding>,
    pub provider_tool: Option<super::HostedProviderToolBinding>,
    pub access_tool: Option<super::HostedAccessToolBinding>,
    /// Control-plane correlation for an already admitted turn; not launch data.
    pub recovered_prompt: Option<crate::PromptIdentity>,
    /// Control-plane admitted context replay; input stays restricted to this
    /// operation until its tools and settings have been acknowledged.
    pub recovered_context_sequence: Option<u64>,
}

/// Provider-specific rendering needed by the otherwise shared hosted adapter.
pub trait HostedAgentRuntime: Send + Sync {
    fn home_plan(&self, request: &AgentHomeRequest) -> Result<AgentHomePlan>;
    fn launch(&self, request: &AgentOpenRequest) -> Result<AgentLaunch>;

    fn open_settings(&self, request: &AgentOpenRequest) -> Result<HostedAgentOpenSettings> {
        Ok(HostedAgentOpenSettings {
            coordinated_ingress: false,
            retained_json: false,
            bounded_raw_output: false,
            bounded_permission_state: false,
            output_json_budget: false,
            shared_runtime_output: false,
            already_initialized_error: None,
            open_completion: None,
            context_refresh: None,
            permission_mode: None,
            own_credential: request.options.own_credential,
            channel_tool: None,
            provider_tool: None,
            access_tool: None,
            recovered_prompt: None,
            recovered_context_sequence: None,
        })
    }
}

/// Provider lifecycle and supervisor transport used by a hosted adapter.
#[async_trait::async_trait]
pub trait HostedBackend: Send + Sync {
    fn stale_turn_producer(
        &self,
        _handle: &RuntimeHandle,
    ) -> Option<&dyn crate::stale_turn::StaleTurnProducer> {
        None
    }

    /// Snapshot only: the caller must exclude all other binding-cache writers until done.
    fn startup_binding_headroom(&self, _handles: &[RuntimeHandle]) -> Option<usize> {
        None
    }
    fn desktop_ownership(
        &self,
        _: &RuntimeHandle,
    ) -> Option<&dyn crate::desktop_ownership::DesktopOwnedRuntime> {
        None
    }
    /// Present only when this backend supplies an independent native proof.
    fn capacity_recovery_transport(
        &self,
        _handle: &RuntimeHandle,
    ) -> Option<&dyn super::CapacityRecoveryNativeTransport> {
        None
    }

    async fn open_desktop(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<crate::DesktopConnectionHandle> {
        Err(crate::AdapterError::Unsupported(
            "runtime desktop is unavailable".into(),
        ))
    }

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
    async fn select_native_canary_pool(
        &self,
        _request: &ProviderSandboxRequest,
        _image: &crate::RuntimeImage,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<crate::agent_sandbox::NativePoolBinding> {
        Err(crate::AdapterError::Unsupported(
            "native canary allocation requires a native backend".into(),
        ))
    }

    async fn validate_funding(
        &self,
        request: &ProviderSandboxRequest,
        _existing: Option<&RuntimeHandle>,
    ) -> Result<()> {
        if request.funded_until.is_some() {
            return Err(crate::AdapterError::Unsupported(
                "funded provider deadlines are unavailable".into(),
            ));
        }
        Ok(())
    }
    async fn set_runtime_deadline(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
        _until: u64,
    ) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "funded provider deadlines are unavailable".into(),
        ))
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
    async fn challenge_retirement_receiver(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
        _receiver: &crate::receiver_failure::RetirementReceiver,
    ) -> Result<bool> {
        Err(crate::AdapterError::Unsupported(
            "native retirement challenge is unavailable".into(),
        ))
    }
    async fn retire_unreachable(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
        _receiver: &crate::RetirementPod,
    ) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "verified provider retirement is unavailable".into(),
        ))
    }
    async fn verify_retired(
        &self,
        _handle: &RuntimeHandle,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "provider retirement finality is unavailable".into(),
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
    fn retained_startup_policy(&self) -> Option<&dyn super::HostedRetainedStartupPolicy> {
        None
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

    /// A persisted home supplies identity only; adoption still proves live authority.
    async fn adopt_with_home(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<Option<RuntimeObservation>> {
        self.adopt(handle, request).await
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

    /// Opt-in preparation must gate all binding, assignment and status dispatch.
    async fn inspect_native_runtime_after(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
        admission: &crate::InspectionAdmission,
    ) -> Result<Option<crate::RuntimeInspection>> {
        admission.wait().await?;
        self.inspect_native_runtime(handle, request, home).await
    }

    /// Fresh observation only; this must never bind or start a runtime process.
    async fn inspect_nonexecuting_runtime(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<Option<super::NonexecutingRuntimeProof>> {
        Err(crate::AdapterError::Unsupported(
            "nonexecuting runtime inspection".into(),
        ))
    }

    /// Replay the complete provider proof immediately before the admitted action.
    async fn verify_nonexecuting_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
        proof: &super::NonexecutingRuntimeProof,
    ) -> Result<()> {
        if self
            .inspect_nonexecuting_runtime(handle, request, home)
            .await?
            .as_ref()
            != Some(proof)
        {
            return Err(crate::AdapterError::Rejected(
                "nonexecuting runtime evidence changed".into(),
            ));
        }
        Ok(())
    }

    /// Revalidate the complete fresh observation immediately before a suspension
    /// CAS using its original object UID/resourceVersion. No generic fallback.
    async fn suspend_nonexecuting_runtime(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
        _proof: &super::NonexecutingRuntimeProof,
    ) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "nonexecuting runtime suspension".into(),
        ))
    }

    /// Internal bounded provider maintenance on the exact existing runtime.
    /// Only the lifecycle adapter may authorize the provider-rendered command.
    /// Implementations must fence resource identity, bound I/O and duration,
    /// and require successful termination before returning stdout.
    async fn run_recovery_command(
        &self,
        _handle: &RuntimeHandle,
        _command: &RuntimeRecoveryCommand,
    ) -> Result<Vec<u8>> {
        Err(crate::AdapterError::Unsupported(
            "runtime recovery commands are not supported".into(),
        ))
    }

    /// Admit command transport, bounded output and errors before constructing
    /// the recovery future. Preserve the legacy identity/termination checks;
    /// allocation custody never authorizes a maintenance command.
    fn run_recovery_command_admitted<'a>(
        &'a self,
        _handle: &'a RuntimeHandle,
        _command: crate::ingress_budget::OwnedWork<RuntimeRecoveryCommand>,
        _policy: &crate::ingress_budget::IngressOwnershipPolicy,
    ) -> std::result::Result<
        crate::ingress_budget::AdmittedOperation<'a, Vec<u8>>,
        crate::ingress_budget::AdmittedError,
    > {
        Err(crate::ingress_budget::AdmittedError::Unsupported(
            "hosted backend has no admitted recovery implementation",
        ))
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

    /// Synchronous admission port: no legacy future construction or transport
    /// fallback. The producer must validate request ownership against `policy`
    /// and cover transport/decode/working overlap before returning owned output.
    fn exchange_admitted<'a>(
        &'a self,
        _handle: &'a RuntimeHandle,
        _request: crate::ingress_budget::OwnedWork<RequestFrame>,
        _policy: &crate::ingress_budget::IngressOwnershipPolicy,
    ) -> std::result::Result<
        crate::ingress_budget::AdmittedOperation<'a, ResponseFrame>,
        crate::ingress_budget::AdmittedError,
    > {
        Err(crate::ingress_budget::AdmittedError::Unsupported(
            "hosted backend has no admitted exchange implementation",
        ))
    }

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
