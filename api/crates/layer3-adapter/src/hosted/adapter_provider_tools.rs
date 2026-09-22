use super::HostedAdapter;
use crate::hosted::HostedProviderToolBinding;
use crate::{
    wire::{RequestFrame, ResponseFrame},
    AdapterError, AgentOpenRequest, RuntimeHandle,
};
use std::sync::Arc;

impl HostedAdapter {
    pub fn with_provider_tools(
        mut self,
        source: Arc<dyn crate::hosted::HostedProviderToolSource>,
    ) -> Self {
        self.provider_tools = Some(source);
        self
    }

    pub(super) async fn resolve_provider_tools(
        &self,
        handle: &RuntimeHandle,
        request: &AgentOpenRequest,
        fence: u64,
    ) -> crate::Result<Option<HostedProviderToolBinding>> {
        let Some(source) = &self.provider_tools else {
            return Ok(None);
        };
        let Some(mut binding) = source.resolve(handle, request, fence).await? else {
            return Ok(None);
        };
        let (bound_runtime, bound_request, bound_fence, proxy) = binding.scope();
        let state = self.known_state(handle).await?;
        if bound_runtime != handle
            || bound_request != request
            || bound_fence != fence
            || proxy.sandbox_id != state.provision.sandbox_id
            || proxy.generation != state.provision.generation
        {
            return Err(AdapterError::Rejected(
                "provider tool attachment scope changed".into(),
            ));
        }
        let ResponseFrame::ToolProxyStatus(status) = self
            .exchange(handle, RequestFrame::ToolProxyStatus(proxy.clone()))
            .await?
        else {
            return Err(AdapterError::Rejected(
                "runtime did not prove its tool endpoint".into(),
            ));
        };
        binding.observe(status)?;
        Ok(Some(binding))
    }
}
