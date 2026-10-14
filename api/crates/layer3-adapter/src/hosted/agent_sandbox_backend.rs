use super::{HostedAgentOpenSettings, HostedBackend, HostedControlTransport};
use crate::{
    agent_sandbox::AgentSandboxClient,
    provider::ProviderSandboxRequest,
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    *,
};
use kube::{api::DynamicObject, ResourceExt};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;

pub struct AgentSandboxBackend {
    client: AgentSandboxClient,
    transport: Arc<dyn HostedControlTransport>,
    assignments: std::sync::Mutex<std::collections::BTreeMap<RuntimeHandle, binding::Assignment>>,
}

#[path = "agent_sandbox_binding.rs"]
mod binding;
#[path = "agent_sandbox_context.rs"]
mod context;
use context::observation;

impl AgentSandboxBackend {
    pub fn new(client: AgentSandboxClient, transport: Arc<dyn HostedControlTransport>) -> Self {
        Self {
            client,
            transport,
            assignments: Default::default(),
        }
    }

    async fn bind(&self, handle: &RuntimeHandle, request: &ProviderSandboxRequest) -> Result<()> {
        let mut timing = crate::agent_sandbox::provision_timing::ProvisionTiming::new();
        let pod = self
            .client
            .wait_ready(handle, Duration::from_secs(180))
            .await?;
        timing.completed("wait_ready");
        let result = self.assign_and_bind(handle, &pod, request).await;
        timing.completed("assign_and_bind");
        result
    }
}

#[async_trait::async_trait]
impl HostedBackend for AgentSandboxBackend {
    fn native_pools(&self) -> Option<&dyn crate::agent_sandbox::NativePoolControl> {
        Some(self.client.warm_pools())
    }
    async fn select_native_pool(
        &self,
        request: &ProviderSandboxRequest,
        image: &crate::RuntimeImage,
        home: &crate::HomeVolumeRequest,
    ) -> Result<Option<crate::agent_sandbox::NativePoolBinding>> {
        self.client
            .warm_pools()
            .select_runtime(request, image, home)
            .await
    }
    async fn provision(&self, request: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        let mut timing = crate::agent_sandbox::provision_timing::ProvisionTiming::new();
        let handle = match self.client.provision_pool_runtime(&request).await? {
            Some(handle) => handle,
            None => self.client.create(&request).await?,
        };
        timing.completed("runtime_admission");
        self.bind(&handle, &request).await?;
        timing.completed("runtime_binding");
        Ok(handle)
    }
    async fn provision_with_home(
        &self,
        request: ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<RuntimeHandle> {
        if !home.request.policy.is_native_vct() {
            return self.provision(request).await;
        }
        let handle = self
            .client
            .provision_pool_runtime_with_home(&request, home)
            .await?;
        self.bind(&handle, &request).await?;
        Ok(handle)
    }
    async fn teardown_retained(&self, handle: &RuntimeHandle) -> Result<()> {
        self.teardown(handle).await
    }
    async fn suspend(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.forget_assignment(handle);
        self.client.suspend(handle, request).await?;
        self.transport.retire(handle).await
    }
    async fn verify_suspended(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.client.verify_suspended(handle, request).await
    }
    async fn resume(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.client.resume(handle, previous, next).await?;
        self.bind_resumed(handle, next).await
    }
    async fn resume_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<()> {
        self.client
            .resume_with_home(handle, previous, next, home)
            .await?;
        self.bind_resumed(handle, next).await
    }
    async fn provision_receipted(
        &self,
        request: ProviderSandboxRequest,
        operation_id: &str,
        request_sha256: &str,
    ) -> Result<RuntimeProvisionReceipt> {
        let receipt = self
            .client
            .create_receipted(&request, operation_id, request_sha256)
            .await?;
        self.bind(receipt.handle(), &request).await?;
        Ok(receipt)
    }
    async fn finalize_runtime(
        &self,
        original: &RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> Result<RuntimeFinalityReceipt> {
        self.forget_assignment(original.handle());
        self.transport.retire(original.handle()).await?;
        self.client.finalize(original, operation_id).await
    }
    async fn inventory(&self) -> Result<Vec<RuntimeObservation>> {
        self.client
            .inventory()
            .await?
            .iter()
            .map(observation)
            .collect()
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<Option<RuntimeObservation>> {
        let Some(object) = self.client.validate_request(handle, request).await? else {
            return Ok(None);
        };
        self.bind_adopted(handle, request, &object).await?;
        Ok(Some(observation(&object)?))
    }
    async fn inspect_native_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<RuntimeInspection>> {
        self.inspect_native_context(handle, request, home).await
    }

    async fn reclaim(&self, expected: &RuntimeObservation) -> Result<()> {
        let Some(object) = self.client.get(&expected.handle).await? else {
            return Ok(());
        };
        if observation(&object)? != *expected {
            return Err(AdapterError::Rejected(
                "Sandbox changed before reclamation".into(),
            ));
        }
        self.teardown(&expected.handle).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> Result<ResponseFrame> {
        self.transport.exchange(handle, request).await
    }

    async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> Result<Vec<u8>> {
        self.transport.export_agent_history(handle, agent).await
    }
    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
    ) -> Result<ResponseFrame> {
        self.transport.verify_checkpoint_hold(handle, request).await
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.transport
            .open_agent(handle, launch, resume_conversation, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()> {
        self.forget_assignment(handle);
        self.transport.retire(handle).await?;
        self.client.delete(handle).await
    }
}

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
        let identity = super::deployment::provider_request_identity(request)?;
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
        let identity = super::deployment::provider_request_identity(request)?;
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

#[cfg(test)]
#[path = "runtime_history_tests.rs"]
mod history_tests;

#[cfg(test)]
#[path = "runtime_routing_tests.rs"]
mod routing_tests;
