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
    pub(super) fn retirement_backend(
        &self,
        handle: &RuntimeHandle,
        home: &HomeVolumeAttachment,
    ) -> Result<&dyn HostedBackend> {
        self.verify_retained_home_route(handle, &home.request)?;
        self.for_handle(handle)
    }

    pub(super) fn check_retained_home_route(
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
