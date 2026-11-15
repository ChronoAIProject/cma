//! Provider lifecycle and transport share the same durable mutation barrier.
use super::{EffectAuthority, EffectIntent, dispatch, pools::GuardedPools};
use crate::{
    hosted::{HostedAgentOpenSettings, HostedBackend, RuntimeRecoveryCommand},
    provider::ProviderSandboxRequest,
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    *,
};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct GuardedBackend {
    inner: Arc<dyn HostedBackend>,
    authority: Arc<dyn EffectAuthority>,
    pools: GuardedPools,
}
impl GuardedBackend {
    pub fn new(inner: Arc<dyn HostedBackend>, authority: Arc<dyn EffectAuthority>) -> Self {
        Self {
            pools: GuardedPools::new(inner.clone(), authority.clone()),
            inner,
            authority,
        }
    }
}

#[async_trait::async_trait]
impl HostedBackend for GuardedBackend {
    fn startup_binding_headroom(&self, _handles: &[RuntimeHandle]) -> Option<usize> {
        self.inner.startup_binding_headroom(_handles)
    }
    fn desktop_ownership(
        &self,
        _handle: &RuntimeHandle,
    ) -> Option<&dyn crate::desktop_ownership::DesktopOwnedRuntime> {
        None
    }
    fn capacity_recovery_transport(
        &self,
        _handle: &RuntimeHandle,
    ) -> Option<&dyn crate::hosted::CapacityRecoveryNativeTransport> {
        None
    }
    async fn open_desktop(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<crate::DesktopConnectionHandle> {
        let intent = EffectIntent::new("open_desktop", _handle.0.as_str(), &(&_handle,))?;
        dispatch(&self.authority, intent, self.inner.open_desktop(_handle)).await
    }
    async fn reserve_home_request(
        &self,
        _runtime: &ProviderSandboxRequest,
        home: &crate::HomeVolumeRequest,
    ) -> Result<crate::HomeVolumeRequest> {
        let intent = EffectIntent::new("reserve_home_request", "allocation", &(&_runtime, &home))?;
        dispatch(
            &self.authority,
            intent,
            self.inner.reserve_home_request(_runtime, home),
        )
        .await
    }
    async fn verify_home_route(
        &self,
        _runtime: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<()> {
        self.inner.verify_home_route(_runtime, _home).await
    }
    fn verify_retained_home_route(
        &self,
        _handle: &RuntimeHandle,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<()> {
        self.inner.verify_retained_home_route(_handle, _home)
    }
    fn native_pools(&self) -> Option<&dyn crate::agent_sandbox::NativePoolControl> {
        self.inner
            .native_pools()
            .map(|_| &self.pools as &dyn crate::agent_sandbox::NativePoolControl)
    }
    fn native_pools_for_home(
        &self,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<Option<&dyn crate::agent_sandbox::NativePoolControl>> {
        match self.inner.native_pools_for_home(_home)? {
            None => Ok(None),
            Some(source)
                if self
                    .inner
                    .native_pools()
                    .is_some_and(|default| std::ptr::eq(source, default)) =>
            {
                Ok(Some(&self.pools))
            }
            Some(_) => Err(AdapterError::Rejected(
                "provider effect authority cannot adopt a different pool route".into(),
            )),
        }
    }
    async fn select_native_pool(
        &self,
        _request: &ProviderSandboxRequest,
        _image: &crate::RuntimeImage,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<Option<crate::agent_sandbox::NativePoolBinding>> {
        let intent = EffectIntent::new(
            "select_native_pool",
            "allocation",
            &(&_request, &_image, &_home),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner.select_native_pool(_request, _image, _home),
        )
        .await
    }
    async fn validate_funding(
        &self,
        request: &ProviderSandboxRequest,
        _existing: Option<&RuntimeHandle>,
    ) -> Result<()> {
        self.inner.validate_funding(request, _existing).await
    }
    async fn set_runtime_deadline(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
        _until: u64,
    ) -> Result<()> {
        let intent = EffectIntent::new(
            "set_runtime_deadline",
            _handle.0.as_str(),
            &(&_handle, &_request, &_until),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner.set_runtime_deadline(_handle, _request, _until),
        )
        .await
    }
    async fn provision(&self, request: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        let intent = EffectIntent::new("provision", "allocation", &(&request,))?;
        dispatch(&self.authority, intent, self.inner.provision(request)).await
    }
    async fn provision_with_home(
        &self,
        request: ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<RuntimeHandle> {
        let intent = EffectIntent::new("provision_with_home", "allocation", &(&request, &_home))?;
        dispatch(
            &self.authority,
            intent,
            self.inner.provision_with_home(request, _home),
        )
        .await
    }
    async fn teardown_retained(&self, _handle: &RuntimeHandle) -> Result<()> {
        let intent = EffectIntent::new("teardown_retained", _handle.0.as_str(), &(&_handle,))?;
        dispatch(
            &self.authority,
            intent,
            self.inner.teardown_retained(_handle),
        )
        .await
    }
    async fn suspend(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
    ) -> Result<()> {
        let intent = EffectIntent::new("suspend", _handle.0.as_str(), &(&_handle, &_request))?;
        dispatch(
            &self.authority,
            intent,
            self.inner.suspend(_handle, _request),
        )
        .await
    }
    async fn verify_suspended(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.inner.verify_suspended(_handle, _request).await
    }
    async fn resume(
        &self,
        _handle: &RuntimeHandle,
        _previous: &ProviderSandboxRequest,
        _next: &ProviderSandboxRequest,
    ) -> Result<()> {
        let intent = EffectIntent::new(
            "resume",
            _handle.0.as_str(),
            &(&_handle, &_previous, &_next),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner.resume(_handle, _previous, _next),
        )
        .await
    }
    async fn resume_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<()> {
        let intent = EffectIntent::new(
            "resume_with_home",
            handle.0.as_str(),
            &(&handle, &previous, &next, &_home),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner.resume_with_home(handle, previous, next, _home),
        )
        .await
    }
    async fn upgrade_with_home(
        &self,
        _handle: &RuntimeHandle,
        _previous: &ProviderSandboxRequest,
        _next: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<()> {
        let intent = EffectIntent::new(
            "upgrade_with_home",
            _handle.0.as_str(),
            &(&_handle, &_previous, &_next, &_home),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner
                .upgrade_with_home(_handle, _previous, _next, _home),
        )
        .await
    }
    async fn provision_receipted(
        &self,
        _request: ProviderSandboxRequest,
        _operation_id: &str,
        _request_sha256: &str,
    ) -> Result<crate::RuntimeProvisionReceipt> {
        let intent = EffectIntent::new(
            "provision_receipted",
            "allocation",
            &(&_request, &_operation_id, &_request_sha256),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner
                .provision_receipted(_request, _operation_id, _request_sha256),
        )
        .await
    }
    async fn finalize_runtime(
        &self,
        _original: &crate::RuntimeProvisionReceipt,
        _operation_id: &str,
    ) -> Result<crate::RuntimeFinalityReceipt> {
        let intent = EffectIntent::new(
            "finalize_runtime",
            "allocation",
            &(&_original, &_operation_id),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner.finalize_runtime(_original, _operation_id),
        )
        .await
    }
    async fn runtime_termination(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<Option<crate::RuntimeTermination>> {
        self.inner.runtime_termination(_handle).await
    }
    async fn inventory(&self) -> Result<Vec<RuntimeObservation>> {
        self.inner.inventory().await
    }
    async fn adopt(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
    ) -> Result<Option<RuntimeObservation>> {
        let intent = EffectIntent::new("adopt", _handle.0.as_str(), &(&_handle, &_request))?;
        dispatch(&self.authority, intent, self.inner.adopt(_handle, _request)).await
    }
    async fn inspect_native_runtime(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<Option<crate::RuntimeInspection>> {
        let intent = EffectIntent::new(
            "inspect_native_runtime",
            _handle.0.as_str(),
            &(&_handle, &_request, &_home),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner.inspect_native_runtime(_handle, _request, _home),
        )
        .await
    }
    async fn inspect_native_runtime_after(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
        admission: &crate::InspectionAdmission,
    ) -> Result<Option<crate::RuntimeInspection>> {
        let intent = EffectIntent::new(
            "inspect_native_runtime_after",
            handle.0.as_str(),
            &(&handle, &request, &home),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner
                .inspect_native_runtime_after(handle, request, home, admission),
        )
        .await
    }
    async fn inspect_nonexecuting_runtime(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
    ) -> Result<Option<crate::hosted::NonexecutingRuntimeProof>> {
        self.inner
            .inspect_nonexecuting_runtime(_handle, _request, _home)
            .await
    }
    async fn verify_nonexecuting_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
        proof: &crate::hosted::NonexecutingRuntimeProof,
    ) -> Result<()> {
        self.inner
            .verify_nonexecuting_runtime(handle, request, home, proof)
            .await
    }
    async fn suspend_nonexecuting_runtime(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
        _home: &crate::HomeVolumeAttachment,
        _proof: &crate::hosted::NonexecutingRuntimeProof,
    ) -> Result<()> {
        let intent = EffectIntent::new(
            "suspend_nonexecuting_runtime",
            _handle.0.as_str(),
            &(&_handle, &_request, &_home, &_proof),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner
                .suspend_nonexecuting_runtime(_handle, _request, _home, _proof),
        )
        .await
    }
    async fn run_recovery_command(
        &self,
        _handle: &RuntimeHandle,
        _command: &RuntimeRecoveryCommand,
    ) -> Result<Vec<u8>> {
        let intent = EffectIntent::new(
            "run_recovery_command",
            _handle.0.as_str(),
            &(&_handle, &_command.program, &_command.args, &_command.input),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner.run_recovery_command(_handle, _command),
        )
        .await
    }
    async fn reclaim(&self, _observation: &RuntimeObservation) -> Result<()> {
        let intent = EffectIntent::new("reclaim", "allocation", &(&_observation,))?;
        dispatch(&self.authority, intent, self.inner.reclaim(_observation)).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> Result<ResponseFrame> {
        let intent = EffectIntent::new("exchange", handle.0.as_str(), &(&handle, &request))?;
        dispatch(
            &self.authority,
            intent,
            self.inner.exchange(handle, request),
        )
        .await
    }
    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &crate::CheckpointHoldRequest,
    ) -> Result<ResponseFrame> {
        let intent = EffectIntent::new(
            "verify_checkpoint_hold",
            handle.0.as_str(),
            &(&handle, &request),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner.verify_checkpoint_hold(handle, request),
        )
        .await
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        let intent = EffectIntent::new(
            "open_agent",
            handle.0.as_str(),
            &(&handle, &launch, &resume_conversation),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.inner
                .open_agent(handle, launch, resume_conversation, settings, updates),
        )
        .await
    }
    async fn export_agent_history(
        &self,
        _handle: &RuntimeHandle,
        _agent: &crate::AgentSlot,
    ) -> Result<Vec<u8>> {
        self.inner.export_agent_history(_handle, _agent).await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()> {
        let intent = EffectIntent::new("teardown", handle.0.as_str(), &(&handle,))?;
        dispatch(&self.authority, intent, self.inner.teardown(handle)).await
    }
}
