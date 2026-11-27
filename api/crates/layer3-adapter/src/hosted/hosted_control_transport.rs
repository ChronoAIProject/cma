use super::HostedAgentOpenSettings;
use crate::ingress_budget::{AdmittedError, AdmittedOperation, IngressOwnershipPolicy, OwnedWork};
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeHandle};
use tokio::sync::mpsc;

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

    /// Dispatch a checked read only to the observed native Pod and assignment.
    async fn exchange_bound_file_read(
        &self,
        _handle: &RuntimeHandle,
        _pod: &crate::agent_sandbox::PodBinding,
        _request_sha256: &str,
        _request: crate::AgentFileReadRequest,
    ) -> crate::Result<Option<ResponseFrame>> {
        Ok(None)
    }

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
