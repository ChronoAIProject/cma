use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::ingress_budget::{
    AdmissionRefusal, AdmittedError, AdmittedOperation, IngressOwnershipPolicy, OwnedWork,
};

use super::deployment::{
    provider_metadata_identity, provider_request_identity, ProviderIdentity,
    CONTROL_PLANE_METADATA, SANDBOX_METADATA,
};
use super::{
    CheckpointArchiveStoreFactory, HostedAgentOpenSettings, HostedBackend,
    ProviderHostedControlTransport,
};
use crate::provider::{
    ProviderDeleteTarget, ProviderSandbox, ProviderSandboxRequest, SandboxClient,
};
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeHandle,
    RuntimeLifecycle, RuntimeObservation,
};

const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(180);
const DEFAULT_READY_POLL: Duration = Duration::from_millis(500);

/// Runtime control-link implementation paired with the provider lifecycle.
#[async_trait::async_trait]
pub trait HostedControlTransport: Send + Sync {
    fn stale_turn_producer(&self) -> Option<&dyn crate::stale_turn::StaleTurnProducer> {
        None
    }

    async fn challenge_retirement_receiver(
        &self,
        _handle: &RuntimeHandle,
        _binding: &crate::agent_sandbox::PodBinding,
        _owner: &crate::owner_protocol::TransportOwner,
    ) -> crate::Result<bool> {
        Err(crate::AdapterError::Unsupported(
            "native retirement challenge is unavailable".into(),
        ))
    }

    fn desktop_ownership(&self) -> Option<&dyn crate::desktop_ownership::DesktopOwnedRuntime> {
        None
    }
    async fn open_desktop(
        &self,
        _handle: &RuntimeHandle,
    ) -> crate::Result<crate::DesktopConnectionHandle> {
        Err(crate::AdapterError::Unsupported(
            "runtime desktop is unavailable".into(),
        ))
    }

    /// Optional native status exchange whose fresh proof is made after checkout.
    /// None retains the original backend inspection and transport behavior.
    async fn inspect_native_status(
        &self,
        _inspection: &super::NativeStatusInspection<'_>,
    ) -> crate::Result<Option<crate::RuntimeInspection>> {
        Ok(None)
    }

    async fn verify_retained_startup_policy(
        &self,
        _handle: &RuntimeHandle,
        _readback: &crate::RetainedStartupPolicyReadback<'_>,
    ) -> crate::Result<crate::RuntimeStartupPolicyObservation> {
        Err(crate::AdapterError::Unsupported(
            "retained startup policy readback is unavailable".into(),
        ))
    }

    async fn bind_runtime(&self, _handle: &RuntimeHandle, _sandbox_id: &str) -> crate::Result<()> {
        Ok(())
    }

    /// Exact acknowledged native assignment identity for transport reuse.
    async fn bind_native_runtime(
        &self,
        handle: &RuntimeHandle,
        sandbox_id: &str,
        _request_sha256: &str,
    ) -> crate::Result<()> {
        self.bind_runtime(handle, sandbox_id).await
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame>;

    /// Admit transport, decode and result storage before constructing work.
    /// The default refuses without boxing or invoking the legacy exchange.
    fn exchange_admitted<'a>(
        &'a self,
        _handle: &'a RuntimeHandle,
        _request: OwnedWork<RequestFrame>,
        _policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, ResponseFrame>, AdmittedError> {
        Err(AdmittedError::Unsupported(
            "control transport has no admitted exchange implementation",
        ))
    }

    /// Revalidate a published checkpoint's writer hold without requiring export.
    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &crate::CheckpointHoldRequest,
    ) -> crate::Result<ResponseFrame> {
        self.exchange(handle, RequestFrame::CheckpointForTeardown(request.clone()))
            .await
    }

    async fn export_agent_history(
        &self,
        _handle: &RuntimeHandle,
        _agent: &crate::AgentSlot,
    ) -> crate::Result<Vec<u8>> {
        Err(AdapterError::Unsupported(
            "agent history export is not supported".into(),
        ))
    }

    /// Attach an existing native conversation for capacity recovery. This is
    /// separate from `open_agent` so a runtime cannot launch, replay input or
    /// infer native state from a durable active-turn field.
    async fn reattach_existing_capacity_recovery(
        &self,
        _request: &crate::agent_capacity_recovery::CapacityRecoveryRequest,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<
        (
            ConversationChannel,
            crate::agent_capacity_recovery::CapacityRecoveryConfirmation,
        ),
        crate::agent_capacity_recovery::CapacityRecoveryError,
    > {
        Err(crate::agent_capacity_recovery::CapacityRecoveryError::Unsupported)
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)>;

    async fn retire(&self, handle: &RuntimeHandle) -> crate::Result<()>;
}

/// Production backend: provider lifecycle plus an injected supervisor link.
pub struct ProviderHostedBackend {
    client: SandboxClient,
    transport: Arc<dyn HostedControlTransport>,
    ready_timeout: Duration,
    ready_poll: Duration,
    created_by: String,
}

impl ProviderHostedBackend {
    pub fn new(
        client: SandboxClient,
        transport: Arc<dyn HostedControlTransport>,
        created_by: impl Into<String>,
    ) -> crate::Result<Self> {
        let created_by = created_by.into();
        if created_by.is_empty()
            || created_by.len() > 256
            || created_by.trim() != created_by
            || created_by.chars().any(char::is_control)
        {
            return Err(AdapterError::Rejected(
                "provider inventory requires a control-plane identity".into(),
            ));
        }
        Ok(Self {
            client,
            transport,
            ready_timeout: DEFAULT_READY_TIMEOUT,
            ready_poll: DEFAULT_READY_POLL,
            created_by,
        })
    }

    pub fn production(
        client: SandboxClient,
        archive_factory: Arc<dyn CheckpointArchiveStoreFactory>,
        created_by: impl Into<String>,
    ) -> crate::Result<Self> {
        let transport = ProviderHostedControlTransport::new(client.clone(), archive_factory)?;
        Self::new(client, Arc::new(transport), created_by)
    }

    #[must_use]
    pub fn with_readiness(mut self, timeout: Duration, poll: Duration) -> Self {
        self.ready_timeout = timeout;
        self.ready_poll = poll;
        self
    }

    pub fn client(&self) -> &SandboxClient {
        &self.client
    }

    fn require_creator(&self, request: &ProviderSandboxRequest) -> crate::Result<()> {
        if request.metadata.get(CONTROL_PLANE_METADATA) != Some(&self.created_by) {
            return Err(identity_mismatch());
        }
        Ok(())
    }

    async fn scoped_inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        creator_inventory(&self.created_by, self.client.list().await?)
    }

    async fn await_running(
        &self,
        request: &ProviderSandboxRequest,
        initial: crate::provider::ProviderSandbox,
    ) -> crate::Result<RuntimeHandle> {
        let handle = RuntimeHandle(initial.id.clone());
        if initial.is_running() {
            return Ok(handle);
        }
        let deadline = tokio::time::Instant::now() + self.ready_timeout;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(AdapterError::Unavailable(format!(
                    "sandbox {} did not start within {} seconds",
                    handle.0,
                    self.ready_timeout.as_secs()
                )));
            }
            tokio::time::sleep(self.ready_poll).await;
            let sandbox =
                validate_sandbox_identity(request, self.client.get(handle.as_str()).await?)?;
            if sandbox.is_running() {
                return Ok(handle);
            }
        }
    }

    async fn reconcile(
        &self,
        request: &ProviderSandboxRequest,
    ) -> crate::Result<Option<ProviderSandbox>> {
        self.require_creator(request)?;
        matching_sandbox(request, self.client.list().await?)
    }
}

#[async_trait::async_trait]
impl HostedBackend for ProviderHostedBackend {
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

    async fn open_desktop(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<crate::DesktopConnectionHandle> {
        self.transport.open_desktop(handle).await
    }

    async fn provision_receipted(
        &self,
        mut request: ProviderSandboxRequest,
        operation_id: &str,
        request_sha256: &str,
    ) -> crate::Result<crate::RuntimeProvisionReceipt> {
        let reclaimer = self.client.finality_reclaimer()?;
        reclaimer.require_finality_scope()?;
        let sandbox_id = provider_request_identity(&request)?.sandbox_id;
        if self.reconcile(&request).await?.is_some() {
            return Err(AdapterError::Unavailable(
                "existing runtime cannot acquire a receipt for this dispatch".into(),
            ));
        }
        let marker = crate::provider::dispatch_marker(operation_id)?;
        let pre_dispatch = reclaimer.observe_before_dispatch(operation_id).await?;
        request
            .metadata
            .insert(crate::provider::DISPATCH_LABEL.to_owned(), marker.clone());
        // Exactly one POST. A failed or lost response cannot authorize a receipt.
        let (created, response_sha256) = self.client.create_receipted(&request).await?;
        let created = validate_sandbox_identity(&request, created)?;
        if request
            .metadata
            .iter()
            .any(|(key, value)| created.metadata.get(key) != Some(value))
        {
            return Err(identity_mismatch());
        }
        let handle = RuntimeHandle(created.id.clone());
        let original = reclaimer
            .capture_for_binding(
                &handle,
                operation_id,
                request_sha256,
                &response_sha256,
                &request.metadata,
                pre_dispatch,
            )
            .await?;
        let handle = self.await_running(&request, created).await?;
        reclaimer.require_bound_runtime(&original).await?;
        self.transport.bind_runtime(&handle, &sandbox_id).await?;
        Ok(original)
    }

    async fn finalize_runtime(
        &self,
        original: &crate::RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> crate::Result<crate::RuntimeFinalityReceipt> {
        let reclaimer = self.client.finality_reclaimer()?;
        reclaimer.require_bound_runtime(original).await?;
        self.transport.retire(original.handle()).await?;
        reclaimer
            .finalize_bound_runtime(original, operation_id)
            .await
    }

    async fn provision(&self, request: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        let sandbox_id = provider_request_identity(&request)?.sandbox_id;
        let sandbox = match self.reconcile(&request).await? {
            Some(sandbox) => sandbox,
            None => match self.client.create(&request).await {
                Ok(sandbox) => {
                    let created = validate_sandbox_identity(&request, sandbox)?;
                    match self.reconcile(&request).await? {
                        Some(reconciled) if reconciled.id == created.id => reconciled,
                        Some(_) => return Err(identity_mismatch()),
                        None => created,
                    }
                }
                Err(create_error) => match self.reconcile(&request).await? {
                    Some(sandbox) => sandbox,
                    None => return Err(create_error.into()),
                },
            },
        };
        let handle = self.await_running(&request, sandbox).await?;
        self.transport.bind_runtime(&handle, &sandbox_id).await?;
        Ok(handle)
    }

    async fn inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        self.scoped_inventory().await
    }

    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> crate::Result<Option<RuntimeObservation>> {
        self.require_creator(request)?;
        let expected = provider_request_identity(request)?;
        let observation = select_adoption(handle, &expected, self.scoped_inventory().await?)?;
        if observation.is_some() {
            self.transport
                .bind_runtime(handle, &expected.sandbox_id)
                .await?;
        }
        Ok(observation)
    }

    async fn reclaim(&self, observation: &RuntimeObservation) -> crate::Result<()> {
        let Some(target) = self
            .client
            .capture_delete_target(&observation.handle)
            .await?
        else {
            return Ok(());
        };
        let current = self
            .scoped_inventory()
            .await?
            .into_iter()
            .find(|current| current.handle == observation.handle);
        if current.as_ref() != Some(observation) {
            return Err(identity_mismatch());
        }
        self.transport.retire(&observation.handle).await?;
        self.client.delete_captured(target).await?;
        Ok(())
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        self.transport.exchange(handle, request).await
    }

    fn exchange_admitted<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        request: OwnedWork<RequestFrame>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, ResponseFrame>, AdmittedError> {
        if !request.belongs_to(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "provider exchange request ownership",
            }
            .into());
        }
        self.transport.exchange_admitted(handle, request, policy)
    }

    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &crate::CheckpointHoldRequest,
    ) -> crate::Result<ResponseFrame> {
        self.transport.verify_checkpoint_hold(handle, request).await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.transport
            .open_agent(handle, launch, resume_conversation, settings, updates)
            .await
    }

    async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &crate::AgentSlot,
    ) -> crate::Result<Vec<u8>> {
        self.transport.export_agent_history(handle, agent).await
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        // Capture immutable provider authority before waiting on local link
        // retirement. A reused provider name can never redirect this delete.
        let target: Option<ProviderDeleteTarget> =
            self.client.capture_delete_target(handle).await?;
        self.transport.retire(handle).await?;
        if let Some(target) = target {
            self.client.delete_captured(target).await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl super::CapacityRecoveryNativeTransport for ProviderHostedBackend {
    async fn reattach_existing(
        &self,
        request: &crate::agent_capacity_recovery::CapacityRecoveryRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::agent_capacity_recovery::CapacityRecoveryResult<(
        ConversationChannel,
        crate::agent_capacity_recovery::CapacityRecoveryConfirmation,
    )> {
        self.transport
            .reattach_existing_capacity_recovery(request, updates)
            .await
    }
}

#[path = "provider_identity.rs"]
mod identity;
use identity::*;

#[cfg(test)]
#[path = "provider_backend_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "provider_backend_ingress_tests.rs"]
pub(super) mod ingress_tests;
