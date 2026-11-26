use super::{HostedAgentOpenSettings, HostedBackend, HostedControlTransport};
use crate::binding_diagnostics::{adapter as timed, Stage};
use crate::ingress_budget::{
    AdmissionRefusal, AdmittedError, AdmittedOperation, IngressOwnershipPolicy, OwnedWork,
};
use crate::{
    agent_sandbox::AgentSandboxClient,
    provider::ProviderSandboxRequest,
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    *,
};
use kube::{api::DynamicObject, ResourceExt};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct AgentSandboxBackend {
    client: AgentSandboxClient,
    transport: Arc<dyn HostedControlTransport>,
    startup_breaker: startup_breaker::StartupBreaker,
    assignments: std::sync::Mutex<std::collections::BTreeMap<RuntimeHandle, binding::Assignment>>,
}

#[path = "agent_sandbox_startup_breaker.rs"]
mod startup_breaker;

#[path = "agent_sandbox_binding.rs"]
mod binding;
#[path = "agent_sandbox_context.rs"]
mod context;
#[path = "agent_sandbox_status.rs"]
mod status;
use context::observation;
pub use context::NativeStatusInspection;

impl AgentSandboxBackend {
    pub fn new(client: AgentSandboxClient, transport: Arc<dyn HostedControlTransport>) -> Self {
        Self {
            client,
            transport,
            assignments: Default::default(),
            startup_breaker: Default::default(),
        }
    }
}

#[async_trait::async_trait]
impl HostedBackend for AgentSandboxBackend {
    fn stale_turn_producer(
        &self,
        _: &RuntimeHandle,
    ) -> Option<&dyn crate::stale_turn::StaleTurnProducer> {
        self.transport.stale_turn_producer()
    }

    fn startup_binding_headroom(&self, _handles: &[RuntimeHandle]) -> Option<usize> {
        Some(binding::MAX_ASSIGNMENTS.saturating_sub(self.assignments.lock().unwrap().len()))
    }
    fn desktop_ownership(
        &self,
        _: &RuntimeHandle,
    ) -> Option<&dyn crate::desktop_ownership::DesktopOwnedRuntime> {
        self.transport.desktop_ownership()
    }
    fn capacity_recovery_transport(
        &self,
        _handle: &RuntimeHandle,
    ) -> Option<&dyn super::CapacityRecoveryNativeTransport> {
        Some(self)
    }

    async fn open_desktop(&self, handle: &RuntimeHandle) -> Result<crate::DesktopConnectionHandle> {
        self.transport.open_desktop(handle).await
    }

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
    async fn select_native_canary_pool(
        &self,
        request: &ProviderSandboxRequest,
        image: &crate::RuntimeImage,
        home: &crate::HomeVolumeRequest,
    ) -> Result<crate::agent_sandbox::NativePoolBinding> {
        self.client
            .warm_pools()
            .select_runtime_with_cold_policy(request, image, home, true)
            .await?
            .ok_or_else(|| crate::AdapterError::Rejected("native canary pool unavailable".into()))
    }

    async fn validate_funding(
        &self,
        request: &ProviderSandboxRequest,
        _existing: Option<&RuntimeHandle>,
    ) -> Result<()> {
        if let Some(until) = request.funded_until {
            crate::agent_sandbox::funding::deadline(until)?;
        }
        Ok(())
    }
    async fn set_runtime_deadline(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        until: u64,
    ) -> Result<()> {
        self.client
            .set_runtime_deadline(handle, request, until)
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
        let mut timing = crate::agent_sandbox::provision_timing::ProvisionTiming::new();
        let handle = self
            .client
            .provision_pool_runtime_with_home(&request, home)
            .await?;
        timing.completed("runtime_admission");
        self.bind(&handle, &request).await?;
        timing.completed("runtime_binding");
        Ok(handle)
    }
    async fn challenge_retirement_receiver(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        receiver: &crate::receiver_failure::RetirementReceiver,
    ) -> Result<bool> {
        let binding = self
            .client
            .retirement_receiver(handle, request, home)
            .await?;
        if RetirementPod::try_from(&binding)? != receiver.pod {
            return Err(AdapterError::Rejected("retirement receiver changed".into()));
        }
        let unavailable = self
            .transport
            .challenge_retirement_receiver(handle, &binding, &receiver.owner)
            .await?;
        if self
            .client
            .retirement_receiver(handle, request, home)
            .await?
            != binding
        {
            return Err(AdapterError::Rejected("retirement receiver changed".into()));
        }
        Ok(unavailable)
    }
    async fn retire_verified(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        receiver: Option<&RetirementPod>,
    ) -> Result<()> {
        if let Some(receiver) = receiver {
            self.client
                .retire_verified(handle, request, home, Some(receiver))
                .await?;
        } else {
            self.client.retire_suspended(handle, request, home).await?;
        }
        self.forget_assignment(handle);
        self.transport.retire(handle).await
    }
    async fn verify_retired(
        &self,
        handle: &RuntimeHandle,
        home: &HomeVolumeAttachment,
    ) -> Result<()> {
        self.client.verify_retired(handle, home).await
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
    fn retained_startup_policy(&self) -> Option<&dyn super::HostedRetainedStartupPolicy> {
        Some(self)
    }
    async fn resume(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.client.resume(handle, previous, next).await?;
        if let Some(until) = next.funded_until {
            self.client
                .set_runtime_deadline(handle, next, until)
                .await?;
        }
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
        if let Some(until) = next.funded_until {
            self.client
                .set_runtime_deadline(handle, next, until)
                .await?;
        }
        self.bind_resumed(handle, next).await
    }
    async fn upgrade_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
    ) -> Result<()> {
        self.client
            .upgrade_with_home(handle, previous, next, home)
            .await?;
        if let Some(until) = next.funded_until {
            self.client
                .set_runtime_deadline(handle, next, until)
                .await?;
        }
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
    async fn runtime_termination(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<Option<crate::RuntimeTermination>> {
        self.client.runtime_termination(handle).await
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
        let Some(object) = timed(
            Stage::NativeIdentity,
            self.client.validate_request(handle, request),
        )
        .await?
        else {
            return Ok(None);
        };
        self.bind_adopted(handle, request, &object, None).await?;
        // The first object authorized binding, but readiness and assignment can
        // change while that operation waits. Revalidate the exact provider
        // object before exposing a current Running observation.
        timed(
            Stage::NativeIdentity,
            self.refreshed_adoption_observation(handle, request, None),
        )
        .await
    }
    async fn adopt_with_home(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<RuntimeObservation>> {
        if !home.request.policy.is_native_vct() {
            return self.adopt(handle, request).await;
        }
        self.adopt_persisted_home(handle, request, home).await
    }
    async fn inspect_native_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<RuntimeInspection>> {
        self.inspect_native_context(handle, request, home).await
    }
    async fn inspect_native_runtime_after(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        admission: &crate::InspectionAdmission,
    ) -> Result<Option<RuntimeInspection>> {
        self.inspect_native_context_after(handle, request, home, admission)
            .await
    }

    async fn inspect_nonexecuting_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<super::NonexecutingRuntimeProof>> {
        self.startup_breaker.reset(handle);
        self.client
            .inspect_nonexecuting_runtime(handle, request, home)
            .await
    }

    async fn suspend_nonexecuting_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        proof: &super::NonexecutingRuntimeProof,
    ) -> Result<()> {
        self.startup_breaker.reset(handle);
        self.client
            .suspend_nonexecuting_runtime(handle, request, home, proof)
            .await
    }

    async fn run_recovery_command(
        &self,
        handle: &RuntimeHandle,
        command: &super::RuntimeRecoveryCommand,
    ) -> Result<Vec<u8>> {
        self.client.run_recovery_command(handle, command).await
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
        if let RequestFrame::RuntimeStatus(status) = request {
            return self.exchange_bound_status(handle, status).await;
        }
        self.transport.exchange(handle, request).await
    }

    fn exchange_admitted<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        request: OwnedWork<RequestFrame>,
        policy: &IngressOwnershipPolicy,
    ) -> std::result::Result<AdmittedOperation<'a, ResponseFrame>, AdmittedError> {
        if !request.belongs_to(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "native exchange request ownership",
            }
            .into());
        }
        if matches!(&*request, RequestFrame::RuntimeStatus(_)) {
            // Status must retain exchange_bound_status's assignment and Pod
            // checks. Its native reads do not yet have admitted ports.
            return Err(AdmittedError::Unsupported(
                "native status validation requires admitted reads",
            ));
        }
        self.transport.exchange_admitted(handle, request, policy)
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

#[path = "agent_sandbox_backend_capacity_recovery.rs"]
mod capacity_recovery;

#[path = "runtime_routing_backend.rs"]
mod routing;
pub use routing::{RuntimeBackends, RuntimeRoutingStore};

#[cfg(test)]
#[path = "runtime_history_tests.rs"]
mod history_tests;

#[cfg(test)]
#[path = "runtime_routing_tests.rs"]
mod routing_tests;

#[cfg(test)]
#[path = "agent_sandbox_backend_ingress_tests.rs"]
mod ingress_tests;

#[path = "retained_startup_policy_native.rs"]
mod retained_policy;
