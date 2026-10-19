use super::{
    AgentSandboxPools, NativePoolBinding, NativePoolClaim, NativePoolClaimRequest,
    NativePoolCleanup, NativePoolSpec, NativePoolStatus, Result,
};
use crate::HomeVolumeAttachment;

/// Native pool effects exposed to durable control-plane workers and operators.
#[async_trait::async_trait]
pub trait NativePoolControl: Send + Sync {
    /// Freeze new admission before the caller rechecks its durable allocations.
    async fn begin_retirement(&self, _binding: &NativePoolBinding) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "native pool retirement is unavailable".into(),
        ))
    }
    /// Called only after the durable admission barrier. False means the caller
    /// must resize to zero, or controller drain/definition deletion is pending.
    async fn retire_pool(
        &self,
        _binding: &NativePoolBinding,
        _retained_homes: &[HomeVolumeAttachment],
    ) -> Result<bool> {
        Err(crate::AdapterError::Unsupported(
            "native pool retirement is unavailable".into(),
        ))
    }
    async fn inventory_claims(&self) -> Result<Vec<NativePoolClaim>> {
        Err(crate::AdapterError::Unsupported(
            "native claim inventory is unavailable".into(),
        ))
    }
    async fn reclaim_claim(&self, _claim: &NativePoolClaim) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "native claim reclamation is unavailable".into(),
        ))
    }
    async fn cleanup_retained_home(&self, _home: &HomeVolumeAttachment) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "native home finality is unavailable".into(),
        ))
    }
    fn plan(&self, spec: &NativePoolSpec) -> Result<super::NativePoolConfiguration>;
    async fn configure_planned(
        &self,
        plan: &super::NativePoolConfiguration,
        replicas: u32,
    ) -> Result<NativePoolStatus>;
    async fn inventory(&self) -> Result<Vec<NativePoolStatus>>;
    async fn resize(&self, binding: &NativePoolBinding, replicas: u32) -> Result<NativePoolStatus>;
    async fn claim(&self, request: &NativePoolClaimRequest) -> Result<NativePoolClaim>;
    async fn observe_claim(&self, original: &NativePoolClaim) -> Result<NativePoolClaim>;
    async fn adopt_home(&self, original: &NativePoolClaim) -> Result<HomeVolumeAttachment>;
    async fn plan_cleanup(&self, original: &NativePoolCleanup) -> Result<NativePoolCleanup>;
    async fn cleanup(&self, original: &NativePoolCleanup) -> Result<()>;
}

#[async_trait::async_trait]
impl NativePoolControl for AgentSandboxPools {
    async fn begin_retirement(&self, binding: &NativePoolBinding) -> Result<()> {
        self.begin_retirement(binding).await
    }
    async fn retire_pool(
        &self,
        binding: &NativePoolBinding,
        retained_homes: &[HomeVolumeAttachment],
    ) -> Result<bool> {
        self.retire_pool(binding, retained_homes).await
    }
    async fn inventory_claims(&self) -> Result<Vec<NativePoolClaim>> {
        self.inventory_claims().await
    }
    async fn reclaim_claim(&self, claim: &NativePoolClaim) -> Result<()> {
        self.reclaim_claim(claim).await
    }
    async fn cleanup_retained_home(&self, home: &HomeVolumeAttachment) -> Result<()> {
        self.cleanup_retained_home(home).await
    }
    fn plan(&self, spec: &NativePoolSpec) -> Result<super::NativePoolConfiguration> {
        self.plan(spec)
    }
    async fn configure_planned(
        &self,
        plan: &super::NativePoolConfiguration,
        replicas: u32,
    ) -> Result<NativePoolStatus> {
        self.configure_planned(plan, replicas).await
    }
    async fn inventory(&self) -> Result<Vec<NativePoolStatus>> {
        self.inventory().await
    }
    async fn resize(&self, binding: &NativePoolBinding, replicas: u32) -> Result<NativePoolStatus> {
        self.resize(binding, replicas).await
    }
    async fn claim(&self, request: &NativePoolClaimRequest) -> Result<NativePoolClaim> {
        self.claim(request).await
    }
    async fn observe_claim(&self, original: &NativePoolClaim) -> Result<NativePoolClaim> {
        self.observe_claim(original).await
    }
    async fn adopt_home(&self, original: &NativePoolClaim) -> Result<HomeVolumeAttachment> {
        self.adopt_home(original).await
    }
    async fn plan_cleanup(&self, original: &NativePoolCleanup) -> Result<NativePoolCleanup> {
        self.plan_cleanup(original).await
    }
    async fn cleanup(&self, original: &NativePoolCleanup) -> Result<()> {
        self.cleanup(original).await
    }
}
