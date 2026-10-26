//! Preserve each runtime and home provider through creation and recovery.
use super::*;

/// Persistence port for the provider chosen before a runtime creation request.
#[async_trait::async_trait]
pub trait RuntimeRoutingStore: Send + Sync {
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

impl RuntimeBackends {
    fn require_home_routes(&self, home: &crate::HomeVolumeRequest) -> Result<()> {
        if self.home_routes.is_none() && home.scope.is_some() {
            return Err(AdapterError::Rejected(
                "recorded home storage authority is not configured".into(),
            ));
        }
        Ok(())
    }
    fn select(&self, agent_sandbox: bool) -> Result<&dyn HostedBackend> {
        let backend = if agent_sandbox {
            &self.agent_sandbox
        } else {
            &self.opensandbox
        };
        backend.as_deref().ok_or_else(|| {
            AdapterError::Unavailable("recorded runtime provider is not configured".into())
        })
    }
    async fn allocation_provider(&self, request: &ProviderSandboxRequest) -> Result<bool> {
        let identity = crate::hosted::deployment::provider_request_identity(request)?;
        self.routing
            .select(
                &identity.sandbox_id,
                identity.generation,
                self.allocate_agent_sandbox,
            )
            .await
    }
    fn for_handle(&self, handle: &RuntimeHandle) -> Result<&dyn HostedBackend> {
        self.select(handle.0.starts_with("agent-sandbox:"))
    }
}

#[async_trait::async_trait]
impl HostedBackend for RuntimeBackends {
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
