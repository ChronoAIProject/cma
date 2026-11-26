//! Preserve each runtime and home provider through creation and recovery.
use super::*;
use crate::ingress_budget::{AdmittedError, AdmittedOperation, IngressOwnershipPolicy, OwnedWork};

#[path = "runtime_routing_backend_ingress.rs"]
mod ingress;
#[path = "runtime_routing_provider.rs"]
mod provider;

/// Persistence port for the provider chosen before a runtime creation request.
#[async_trait::async_trait]
pub trait RuntimeRoutingStore: Send + Sync {
    fn require_allocation_image(&self, _image: &str) -> Result<()> {
        Ok(())
    }

    async fn select(&self, sandbox: &str, generation: u64, default_new: bool) -> Result<bool>;
}

/// New allocations use the selected provider. Existing runtime handles retain
/// their provider during rollout and rollback; inventory includes both owners.
pub struct RuntimeBackends {
    pub agent_sandbox: Option<Arc<dyn HostedBackend>>,
    pub opensandbox: Option<Arc<dyn HostedBackend>>,
    pub allocate_agent_sandbox: bool,
    pub routing: Arc<dyn RuntimeRoutingStore>,
    pub home_routes: Option<Arc<crate::RoutedHomeVolumes>>,
}

#[async_trait::async_trait]
impl HostedBackend for RuntimeBackends {
    fn startup_binding_headroom(&self, handles: &[RuntimeHandle]) -> Option<usize> {
        // Count the whole batch against every selected owner; never sum capacities.
        let mut headroom = usize::MAX;
        for native in [false, true] {
            if handles
                .iter()
                .any(|handle| handle.0.starts_with("agent-sandbox:") == native)
            {
                headroom = headroom.min(
                    self.select(native)
                        .ok()?
                        .startup_binding_headroom(handles)?,
                );
            }
        }
        Some(headroom)
    }
    fn desktop_ownership(
        &self,
        handle: &RuntimeHandle,
    ) -> Option<&dyn crate::desktop_ownership::DesktopOwnedRuntime> {
        self.for_handle(handle).ok()?.desktop_ownership(handle)
    }
    fn capacity_recovery_transport(
        &self,
        handle: &RuntimeHandle,
    ) -> Option<&dyn crate::hosted::CapacityRecoveryNativeTransport> {
        self.for_handle(handle)
            .ok()?
            .capacity_recovery_transport(handle)
    }

    fn stale_turn_producer(
        &self,
        handle: &RuntimeHandle,
    ) -> Option<&dyn crate::stale_turn::StaleTurnProducer> {
        self.for_handle(handle).ok()?.stale_turn_producer(handle)
    }

    async fn open_desktop(&self, handle: &RuntimeHandle) -> Result<crate::DesktopConnectionHandle> {
        self.for_handle(handle)?.open_desktop(handle).await
    }

    async fn inspect_nonexecuting_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<super::super::NonexecutingRuntimeProof>> {
        self.verify_retained_home_route(handle, &home.request)?;
        self.for_handle(handle)?
            .inspect_nonexecuting_runtime(handle, request, home)
            .await
    }

    async fn verify_nonexecuting_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        proof: &super::super::NonexecutingRuntimeProof,
    ) -> Result<()> {
        self.verify_retained_home_route(handle, &home.request)?;
        self.for_handle(handle)?
            .verify_nonexecuting_runtime(handle, request, home, proof)
            .await
    }

    async fn suspend_nonexecuting_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        proof: &super::super::NonexecutingRuntimeProof,
    ) -> Result<()> {
        self.verify_retained_home_route(handle, &home.request)?;
        self.for_handle(handle)?
            .suspend_nonexecuting_runtime(handle, request, home, proof)
            .await
    }

    async fn run_recovery_command(
        &self,
        handle: &RuntimeHandle,
        command: &super::super::RuntimeRecoveryCommand,
    ) -> Result<Vec<u8>> {
        self.for_handle(handle)?
            .run_recovery_command(handle, command)
            .await
    }

    fn run_recovery_command_admitted<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        command: OwnedWork<super::super::RuntimeRecoveryCommand>,
        policy: &IngressOwnershipPolicy,
    ) -> std::result::Result<AdmittedOperation<'a, Vec<u8>>, AdmittedError> {
        self.admitted_backend(handle, &command, policy)?
            .run_recovery_command_admitted(handle, command, policy)
    }

    async fn reserve_home_request(
        &self,
        request: &ProviderSandboxRequest,
        home: &crate::HomeVolumeRequest,
    ) -> Result<crate::HomeVolumeRequest> {
        self.require_home_routes(home)?;
        let selected = self.allocation_provider(request).await?;
        self.select(selected)?;
        match &self.home_routes {
            Some(routes) => routes.reserve_for(selected, home),
            None => Ok(home.clone()),
        }
    }
    async fn verify_home_route(
        &self,
        request: &ProviderSandboxRequest,
        home: &crate::HomeVolumeRequest,
    ) -> Result<()> {
        self.require_home_routes(home)?;
        let Some(routes) = &self.home_routes else {
            return Ok(());
        };
        let native = routes.is_native(home)?;
        let identity = crate::hosted::deployment::provider_request_identity(request)?;
        let selected = self
            .routing
            .select(&identity.sandbox_id, identity.generation, native)
            .await?;
        if native != selected && !(home.scope.is_none() && routes.shares_legacy_authority()) {
            return Err(AdapterError::Rejected(
                "runtime provider conflicts with its recorded home authority".into(),
            ));
        }
        self.select(selected)?;
        Ok(())
    }
    fn native_pools(&self) -> Option<&dyn crate::agent_sandbox::NativePoolControl> {
        self.agent_sandbox
            .as_deref()
            .and_then(|backend| backend.native_pools())
    }
    fn verify_retained_home_route(
        &self,
        handle: &RuntimeHandle,
        home: &crate::HomeVolumeRequest,
    ) -> Result<()> {
        self.require_home_routes(home)?;
        if let Some(routes) = &self.home_routes {
            if routes.is_native(home)? != handle.0.starts_with("agent-sandbox:")
                && !(home.scope.is_none() && routes.shares_legacy_authority())
            {
                return Err(AdapterError::Rejected(
                    "retained runtime conflicts with its recorded home authority".into(),
                ));
            }
        }
        self.for_handle(handle)?;
        Ok(())
    }
    fn native_pools_for_home(
        &self,
        home: &crate::HomeVolumeRequest,
    ) -> Result<Option<&dyn crate::agent_sandbox::NativePoolControl>> {
        self.require_home_routes(home)?;
        if let Some(routes) = &self.home_routes {
            if !(routes.is_native(home)?
                || home.scope.is_none() && routes.shares_legacy_authority())
            {
                return Ok(None);
            }
        }
        Ok(self.native_pools())
    }
    async fn select_native_pool(
        &self,
        request: &ProviderSandboxRequest,
        image: &crate::RuntimeImage,
        home: &crate::HomeVolumeRequest,
    ) -> Result<Option<crate::agent_sandbox::NativePoolBinding>> {
        self.require_home_routes(home)?;
        let selected = self.allocation_provider(request).await?;
        self.select(selected)?
            .select_native_pool(request, image, home)
            .await
    }
    async fn select_native_canary_pool(
        &self,
        request: &ProviderSandboxRequest,
        image: &crate::RuntimeImage,
        home: &crate::HomeVolumeRequest,
    ) -> Result<crate::agent_sandbox::NativePoolBinding> {
        self.require_home_routes(home)?;
        if !self.allocation_provider(request).await? {
            return Err(AdapterError::Rejected(
                "runtime canary requires native allocation".into(),
            ));
        }
        self.select(true)?
            .select_native_canary_pool(request, image, home)
            .await
    }

    async fn validate_funding(
        &self,
        request: &ProviderSandboxRequest,
        existing: Option<&RuntimeHandle>,
    ) -> Result<()> {
        if request.funded_until.is_none() {
            return Ok(());
        }
        if let Some(handle) = existing {
            return self
                .for_handle(handle)?
                .validate_funding(request, Some(handle))
                .await;
        }
        let selected = self.allocation_provider(request).await?;
        self.select(selected)?.validate_funding(request, None).await
    }
    async fn set_runtime_deadline(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        until: u64,
    ) -> Result<()> {
        self.for_handle(handle)?
            .set_runtime_deadline(handle, request, until)
            .await
    }

    async fn provision(&self, request: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        let selected = self.allocation_provider(&request).await?;
        self.select(selected)?.provision(request).await
    }
    async fn provision_with_home(
        &self,
        request: ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<RuntimeHandle> {
        let selected = self.allocation_provider(&request).await?;
        self.select(selected)?
            .provision_with_home(request, home)
            .await
    }
    async fn challenge_retirement_receiver(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
        receiver: &crate::receiver_failure::RetirementReceiver,
    ) -> Result<bool> {
        self.verify_retained_home_route(handle, &home.request)?;
        self.for_handle(handle)?
            .challenge_retirement_receiver(handle, request, home, receiver)
            .await
    }
    async fn retire_unreachable(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
        receiver: &crate::RetirementPod,
    ) -> Result<()> {
        self.verify_retained_home_route(handle, &home.request)?;
        self.for_handle(handle)?
            .retire_unreachable(handle, request, home, receiver)
            .await
    }
    async fn verify_retired(
        &self,
        handle: &RuntimeHandle,
        home: &crate::HomeVolumeAttachment,
    ) -> Result<()> {
        self.verify_retained_home_route(handle, &home.request)?;
        self.for_handle(handle)?.verify_retired(handle, home).await
    }
    async fn teardown_retained(&self, handle: &RuntimeHandle) -> Result<()> {
        self.for_handle(handle)?.teardown_retained(handle).await
    }
    async fn suspend(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.for_handle(handle)?.suspend(handle, request).await
    }
    async fn verify_suspended(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.for_handle(handle)?
            .verify_suspended(handle, request)
            .await
    }
    fn retained_startup_policy(&self) -> Option<&dyn crate::hosted::HostedRetainedStartupPolicy> {
        Some(self)
    }
    async fn resume(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.for_handle(handle)?
            .resume(handle, previous, next)
            .await
    }
    async fn resume_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<()> {
        self.for_handle(handle)?
            .resume_with_home(handle, previous, next, home)
            .await
    }
    async fn upgrade_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
    ) -> Result<()> {
        self.for_handle(handle)?
            .upgrade_with_home(handle, previous, next, home)
            .await
    }
    async fn provision_receipted(
        &self,
        request: ProviderSandboxRequest,
        op: &str,
        digest: &str,
    ) -> Result<RuntimeProvisionReceipt> {
        let selected = self.allocation_provider(&request).await?;
        self.select(selected)?
            .provision_receipted(request, op, digest)
            .await
    }
    async fn finalize_runtime(
        &self,
        original: &RuntimeProvisionReceipt,
        op: &str,
    ) -> Result<RuntimeFinalityReceipt> {
        self.for_handle(original.handle())?
            .finalize_runtime(original, op)
            .await
    }
    async fn runtime_termination(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<Option<crate::RuntimeTermination>> {
        self.for_handle(handle)?.runtime_termination(handle).await
    }

    async fn inventory(&self) -> Result<Vec<RuntimeObservation>> {
        let mut all = Vec::new();
        for backend in [&self.agent_sandbox, &self.opensandbox]
            .into_iter()
            .flatten()
        {
            all.extend(backend.inventory().await?);
        }
        Ok(all)
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<Option<RuntimeObservation>> {
        self.for_handle(handle)?.adopt(handle, request).await
    }
    async fn adopt_with_home(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<RuntimeObservation>> {
        self.for_handle(handle)?
            .adopt_with_home(handle, request, home)
            .await
    }
    async fn inspect_native_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<RuntimeInspection>> {
        self.for_handle(handle)?
            .inspect_native_runtime(handle, request, home)
            .await
    }
    async fn inspect_native_runtime_after(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        admission: &crate::InspectionAdmission,
    ) -> Result<Option<RuntimeInspection>> {
        self.for_handle(handle)?
            .inspect_native_runtime_after(handle, request, home, admission)
            .await
    }

    async fn reclaim(&self, observation: &RuntimeObservation) -> Result<()> {
        self.for_handle(&observation.handle)?
            .reclaim(observation)
            .await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> Result<ResponseFrame> {
        self.for_handle(handle)?.exchange(handle, request).await
    }

    fn exchange_admitted<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        request: OwnedWork<RequestFrame>,
        policy: &IngressOwnershipPolicy,
    ) -> std::result::Result<AdmittedOperation<'a, ResponseFrame>, AdmittedError> {
        self.admitted_backend(handle, &request, policy)?
            .exchange_admitted(handle, request, policy)
    }
    async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> Result<Vec<u8>> {
        self.for_handle(handle)?
            .export_agent_history(handle, agent)
            .await
    }
    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
    ) -> Result<ResponseFrame> {
        self.for_handle(handle)?
            .verify_checkpoint_hold(handle, request)
            .await
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.for_handle(handle)?
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()> {
        self.for_handle(handle)?.teardown(handle).await
    }
}

#[cfg(test)]
#[path = "runtime_routing_backend_ingress_tests.rs"]
mod ingress_tests;

#[path = "retained_startup_policy_routing.rs"]
mod retained_policy;
