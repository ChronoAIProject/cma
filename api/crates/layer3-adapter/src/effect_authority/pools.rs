//! Every escaped capability retains the same durable effect authority.
use super::{EffectAuthority, EffectIntent, dispatch};
use crate::*;
use crate::{
    agent_sandbox::{
        NativePoolBinding, NativePoolClaim, NativePoolClaimRequest, NativePoolCleanup,
        NativePoolControl, NativePoolSpec, NativePoolStatus,
    },
    hosted::HostedBackend,
};
use std::sync::Arc;
pub(super) struct GuardedPools {
    backend: Arc<dyn HostedBackend>,
    authority: Arc<dyn EffectAuthority>,
}
impl GuardedPools {
    pub(super) fn new(
        backend: Arc<dyn HostedBackend>,
        authority: Arc<dyn EffectAuthority>,
    ) -> Self {
        Self { backend, authority }
    }
    fn source(&self) -> Result<&dyn NativePoolControl> {
        self.backend
            .native_pools()
            .ok_or_else(|| AdapterError::Rejected("native pool effect route disappeared".into()))
    }
}
#[async_trait::async_trait]
impl NativePoolControl for GuardedPools {
    async fn begin_retirement(&self, _binding: &NativePoolBinding) -> Result<()> {
        let intent = EffectIntent::new("begin_retirement", _binding.name.as_str(), &(&_binding,))?;
        dispatch(
            &self.authority,
            intent,
            self.source()?.begin_retirement(_binding),
        )
        .await
    }
    async fn drain_pool(
        &self,
        _binding: &NativePoolBinding,
        _retained_homes: &[HomeVolumeAttachment],
    ) -> Result<()> {
        let intent = EffectIntent::new(
            "drain_pool",
            _binding.name.as_str(),
            &(&_binding, &_retained_homes),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.source()?.drain_pool(_binding, _retained_homes),
        )
        .await
    }
    async fn retire_pool(
        &self,
        _binding: &NativePoolBinding,
        _retained_homes: &[HomeVolumeAttachment],
    ) -> Result<bool> {
        let intent = EffectIntent::new(
            "retire_pool",
            _binding.name.as_str(),
            &(&_binding, &_retained_homes),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.source()?.retire_pool(_binding, _retained_homes),
        )
        .await
    }
    async fn inventory_claims(&self) -> Result<Vec<NativePoolClaim>> {
        self.source()?.inventory_claims().await
    }
    fn claim_inventory_cursor(&self) -> Option<Box<dyn crate::InventoryCursor<NativePoolClaim>>> {
        self.backend.native_pools()?.claim_inventory_cursor()
    }
    async fn reclaim_claim(&self, _claim: &NativePoolClaim) -> Result<()> {
        let intent = EffectIntent::new("reclaim_claim", _claim.name.as_str(), &(&_claim,))?;
        dispatch(
            &self.authority,
            intent,
            self.source()?.reclaim_claim(_claim),
        )
        .await
    }
    async fn cleanup_retained_home(&self, _home: &HomeVolumeAttachment) -> Result<()> {
        let intent = EffectIntent::new("cleanup_retained_home", "pool", &(&_home,))?;
        dispatch(
            &self.authority,
            intent,
            self.source()?.cleanup_retained_home(_home),
        )
        .await
    }
    fn plan(&self, spec: &NativePoolSpec) -> Result<crate::agent_sandbox::NativePoolConfiguration> {
        self.source()?.plan(spec)
    }
    async fn configure_planned(
        &self,
        plan: &crate::agent_sandbox::NativePoolConfiguration,
        replicas: u32,
    ) -> Result<NativePoolStatus> {
        let intent = EffectIntent::new("configure_planned", "pool", &(&plan, &replicas))?;
        dispatch(
            &self.authority,
            intent,
            self.source()?.configure_planned(plan, replicas),
        )
        .await
    }
    async fn inventory(&self) -> Result<Vec<NativePoolStatus>> {
        self.source()?.inventory().await
    }
    async fn resize(&self, binding: &NativePoolBinding, replicas: u32) -> Result<NativePoolStatus> {
        let intent = EffectIntent::new("resize", binding.name.as_str(), &(&binding, &replicas))?;
        dispatch(
            &self.authority,
            intent,
            self.source()?.resize(binding, replicas),
        )
        .await
    }
    async fn resize_fenced(
        &self,
        _binding: &NativePoolBinding,
        _target: &crate::agent_sandbox::NativePoolTarget,
    ) -> Result<NativePoolStatus> {
        let intent = EffectIntent::new(
            "resize_fenced",
            _binding.name.as_str(),
            &(&_binding, &_target),
        )?;
        dispatch(
            &self.authority,
            intent,
            self.source()?.resize_fenced(_binding, _target),
        )
        .await
    }
    async fn claim(&self, request: &NativePoolClaimRequest) -> Result<NativePoolClaim> {
        let intent = EffectIntent::new("claim", "pool", &(&request,))?;
        dispatch(&self.authority, intent, self.source()?.claim(request)).await
    }
    async fn observe_claim(&self, original: &NativePoolClaim) -> Result<NativePoolClaim> {
        self.source()?.observe_claim(original).await
    }
    async fn adopt_home(&self, original: &NativePoolClaim) -> Result<HomeVolumeAttachment> {
        let intent = EffectIntent::new("adopt_home", original.name.as_str(), &(&original,))?;
        dispatch(&self.authority, intent, self.source()?.adopt_home(original)).await
    }
    async fn plan_cleanup(&self, original: &NativePoolCleanup) -> Result<NativePoolCleanup> {
        self.source()?.plan_cleanup(original).await
    }
    async fn cleanup(&self, original: &NativePoolCleanup) -> Result<()> {
        let intent = EffectIntent::new("cleanup", "pool_cleanup", &(&original,))?;
        dispatch(&self.authority, intent, self.source()?.cleanup(original)).await
    }
}
