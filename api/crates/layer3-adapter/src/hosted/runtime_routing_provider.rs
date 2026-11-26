use super::*;

/// Persistence port for the provider chosen before a runtime creation request.
#[async_trait::async_trait]
pub trait RuntimeRoutingStore: Send + Sync {
    fn require_allocation_image(&self, _image: &str) -> Result<()> {
        Ok(())
    }

    async fn select(&self, sandbox: &str, generation: u64, default_new: bool) -> Result<bool>;
}

impl RuntimeBackends {
    pub(super) fn require_home_routes(&self, home: &crate::HomeVolumeRequest) -> Result<()> {
        if self.home_routes.is_none() && home.scope.is_some() {
            return Err(AdapterError::Rejected(
                "recorded home storage authority is not configured".into(),
            ));
        }
        Ok(())
    }
    pub(super) fn select(&self, agent_sandbox: bool) -> Result<&dyn HostedBackend> {
        let backend = if agent_sandbox {
            &self.agent_sandbox
        } else {
            &self.opensandbox
        };
        backend.as_deref().ok_or_else(|| {
            AdapterError::Unavailable("recorded runtime provider is not configured".into())
        })
    }
    pub(super) async fn allocation_provider(
        &self,
        request: &ProviderSandboxRequest,
    ) -> Result<bool> {
        self.routing.require_allocation_image(&request.image.uri)?;
        let identity = crate::hosted::deployment::provider_request_identity(request)?;
        self.routing
            .select(
                &identity.sandbox_id,
                identity.generation,
                self.allocate_agent_sandbox,
            )
            .await
    }
    pub(super) fn for_handle(&self, handle: &RuntimeHandle) -> Result<&dyn HostedBackend> {
        self.select(handle.0.starts_with("agent-sandbox:"))
    }
}
