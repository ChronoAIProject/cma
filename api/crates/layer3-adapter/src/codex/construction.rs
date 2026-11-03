//! Configure the hosted Codex adapter and its runtime services.
use super::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexSandboxSpec {
    pub deployment: HostedDeployment,
    pub config: CodexConfig,
}

impl CodexAdapter {
    pub fn new(backend: Arc<dyn HostedBackend>, spec: CodexSandboxSpec) -> Self {
        let runtime = Arc::new(CodexRuntime {
            config: spec.config,
        });
        Self {
            hosted: HostedAdapter::new(backend, runtime, spec.deployment),
        }
    }

    pub fn with_backend<B>(backend: B, spec: CodexSandboxSpec) -> Self
    where
        B: HostedBackend + 'static,
    {
        Self::new(Arc::new(backend), spec)
    }

    /// Build the adapter and retain an inspectable hosted test double.
    pub fn in_memory(spec: CodexSandboxSpec) -> (Self, InMemoryHostedBackend) {
        let backend = InMemoryHostedBackend::default();
        (Self::with_backend(backend.clone(), spec), backend)
    }

    pub fn hosted(&self) -> &HostedAdapter {
        &self.hosted
    }

    pub fn with_home_volumes(mut self, store: Arc<dyn crate::HomeVolumeStore>) -> Self {
        self.hosted = self.hosted.with_home_volumes(store);
        self
    }

    pub fn with_provider_tools(
        mut self,
        source: std::sync::Arc<dyn crate::hosted::HostedProviderToolSource>,
    ) -> Self {
        self.hosted = self.hosted.with_provider_tools(source);
        self
    }

    pub fn with_access_tools(
        mut self,
        source: Arc<dyn crate::hosted::HostedAccessToolSource>,
    ) -> Self {
        self.hosted = self.hosted.with_access_tools(source);
        self
    }

    pub fn with_channel_tools(
        mut self,
        source: Arc<dyn crate::hosted::HostedChannelToolSource>,
    ) -> Self {
        self.hosted = self.hosted.with_channel_tools(source);
        self
    }
}
