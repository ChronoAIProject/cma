use super::{
    AgentSandboxPools, NativePoolBinding, NativePoolClaim, NativePoolClaimRequest,
    NativePoolCleanup, NativePoolConfiguration, NativePoolSpec, NativePoolStatus, Result,
};
use crate::HomeVolumeAttachment;

/// Durable allocation evidence for a fenced spare drain.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct NativePoolDrain {
    pub homes: Vec<HomeVolumeAttachment>,
    pub claims: Vec<NativePoolClaim>,
}

/// Native pool effects exposed to durable control-plane workers and operators.
#[async_trait::async_trait]
pub trait NativePoolControl: Send + Sync {
    /// Freeze new admission before the caller rechecks its durable allocations.
    async fn begin_retirement(&self, _binding: &NativePoolBinding) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "native pool retirement is unavailable".into(),
        ))
    }
    /// Drain spare capacity after the marker and a fresh complete durable scan.
    /// Retained homes may still block final definition retirement.
    async fn drain_pool(
        &self,
        _binding: &NativePoolBinding,
        _evidence: &NativePoolDrain,
    ) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "native pool drain is unavailable".into(),
        ))
    }
    /// Request deletion of authoritatively unclaimed members through the
    /// provider's guarded retirement protocol.
    async fn request_spare_drain(
        &self,
        binding: &NativePoolBinding,
        evidence: &NativePoolDrain,
    ) -> Result<()> {
        self.drain_pool(binding, evidence).await
    }
    /// Called only after the durable admission barrier and guarded drain.
    /// False means controller drain or definition deletion is pending.
    async fn retire_pool(
        &self,
        _binding: &NativePoolBinding,
        _retained_homes: &[HomeVolumeAttachment],
    ) -> Result<bool> {
        Err(crate::AdapterError::Unsupported(
            "native pool retirement is unavailable".into(),
        ))
    }
    /// Recover the exact original binding, or prove its definitions and dependants absent.
    async fn recover_retirement_binding(
        &self,
        plan: &NativePoolConfiguration,
    ) -> Result<Option<NativePoolBinding>> {
        let mut matches = self
            .inventory()
            .await?
            .into_iter()
            .map(|status| status.binding)
            .filter(|binding| {
                binding.config_sha256 == plan.config_sha256
                    && binding.spec == plan.spec
                    && binding.pod_policy == plan.pod_policy
            });
        let binding = matches.next().ok_or_else(|| {
            crate::AdapterError::Unsupported("native retirement absence is unavailable".into())
        })?;
        if matches.next().is_some() {
            return Err(crate::AdapterError::Rejected(
                "native retirement binding is ambiguous".into(),
            ));
        }
        Ok(Some(binding))
    }

    async fn inventory_claims(&self) -> Result<Vec<NativePoolClaim>> {
        Err(crate::AdapterError::Unsupported(
            "native claim inventory is unavailable".into(),
        ))
    }
    fn claim_inventory_cursor(&self) -> Option<Box<dyn crate::InventoryCursor<NativePoolClaim>>> {
        None
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
    /// Observe the one immutable plan without creating or resizing its provider resources.
    async fn observe_planned_pool(
        &self,
        configuration: &NativePoolConfiguration,
        _target: &super::NativePoolTarget,
        _fenced: bool,
    ) -> Result<Option<NativePoolStatus>> {
        let mut matching = self.inventory().await?.into_iter().filter(|status| {
            status.binding.spec == configuration.spec
                && status.binding.pod_policy == configuration.pod_policy
                && status.binding.config_sha256 == configuration.config_sha256
        });
        let found = matching.next();
        if matching.next().is_some() {
            return Err(crate::AdapterError::Rejected(
                "native pool plan is ambiguous".into(),
            ));
        }
        Ok(found)
    }
    /// Read the exact pool and its admission barrier from the provider.
    async fn observe_admitting_pool(
        &self,
        _binding: &NativePoolBinding,
        _target: &super::NativePoolTarget,
        _fenced: bool,
    ) -> Result<NativePoolStatus> {
        Err(crate::AdapterError::Unsupported(
            "native pool admission observation is unavailable".into(),
        ))
    }
    async fn resize(&self, binding: &NativePoolBinding, replicas: u32) -> Result<NativePoolStatus>;
    /// Apply the original durable target generation, retaining it across retries.
    /// A provider without receiver fencing cannot serve replica-mode resizing.
    async fn resize_fenced(
        &self,
        _binding: &NativePoolBinding,
        _target: &super::NativePoolTarget,
    ) -> Result<NativePoolStatus> {
        Err(crate::AdapterError::Unsupported(
            "native pool target fencing is unavailable".into(),
        ))
    }
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
    async fn drain_pool(
        &self,
        binding: &NativePoolBinding,
        evidence: &NativePoolDrain,
    ) -> Result<()> {
        self.drain_pool(binding, &evidence.homes, &evidence.claims)
            .await
    }
    async fn request_spare_drain(
        &self,
        binding: &NativePoolBinding,
        evidence: &NativePoolDrain,
    ) -> Result<()> {
        self.request_spare_drain(binding, &evidence.homes, &evidence.claims)
            .await
    }
    async fn retire_pool(
        &self,
        binding: &NativePoolBinding,
        retained_homes: &[HomeVolumeAttachment],
    ) -> Result<bool> {
        self.retire_pool(binding, retained_homes).await
    }
    async fn recover_retirement_binding(
        &self,
        plan: &NativePoolConfiguration,
    ) -> Result<Option<NativePoolBinding>> {
        self.recover_retirement_binding(plan).await
    }

    async fn inventory_claims(&self) -> Result<Vec<NativePoolClaim>> {
        self.inventory_claims().await
    }
    fn claim_inventory_cursor(&self) -> Option<Box<dyn crate::InventoryCursor<NativePoolClaim>>> {
        Some(self.claim_inventory_cursor())
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
    async fn observe_planned_pool(
        &self,
        configuration: &NativePoolConfiguration,
        target: &super::NativePoolTarget,
        fenced: bool,
    ) -> Result<Option<NativePoolStatus>> {
        self.observe_planned_pool(configuration, target, fenced)
            .await
    }
    async fn observe_admitting_pool(
        &self,
        binding: &NativePoolBinding,
        target: &super::NativePoolTarget,
        fenced: bool,
    ) -> Result<NativePoolStatus> {
        self.observe_admitting_pool(binding, target, fenced).await
    }
    async fn resize(&self, binding: &NativePoolBinding, replicas: u32) -> Result<NativePoolStatus> {
        self.resize(binding, replicas).await
    }
    async fn resize_fenced(
        &self,
        binding: &NativePoolBinding,
        target: &super::NativePoolTarget,
    ) -> Result<NativePoolStatus> {
        self.resize_fenced(binding, target).await
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
