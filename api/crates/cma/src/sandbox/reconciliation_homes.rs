//! Reclaim late allocations using fresh warm authority before owned authority.

use layer2_core::sandbox::SandboxStoreError;
use layer2_core::SandboxId;

use super::{corrupt, provider_error, SandboxReconciler};

impl SandboxReconciler {
    pub(super) async fn reconcile_homes(
        &self,
        stopping: &impl Fn() -> bool,
    ) -> Result<(usize, usize), SandboxStoreError> {
        let claims = self.reconcile_native_claims(stopping).await?;
        if stopping() {
            return Ok((claims, 0));
        }
        let Some(homes) = self.adapter.home_volumes() else {
            return Ok((claims, 0));
        };
        let inventory = homes.inventory().await.map_err(provider_error)?;
        let count = inventory.len();
        for observed in inventory {
            if stopping() {
                break;
            }
            self.reconcile_home(&observed).await?;
        }
        tracing::info!(target: "cma_startup", parent: None,
            native_claim_count = claims, home_count = count,
            "API startup home reconciliation counts");
        Ok((claims, count))
    }

    pub(super) async fn reconcile_home(
        &self,
        observed: &layer3_adapter::HomeVolumeAttachment,
    ) -> Result<(), SandboxStoreError> {
        let Some(homes) = self.adapter.home_volumes() else {
            return Ok(());
        };
        observed
            .validate_for(&observed.request.sandbox_id)
            .map_err(provider_error)?;
        if self.store.unowned_home_authority(&observed.request).await? {
            return Ok(());
        }
        let id = SandboxId(observed.request.sandbox_id.clone());
        if let Some(current) = self.store.get_sandbox(&id).await? {
            current
                .validate()
                .map_err(|error| corrupt(&error.to_string()))?;
            if current
                .lifecycle
                .home
                .as_ref()
                .is_some_and(|home| !home.released && home.request == observed.request)
            {
                return Ok(());
            }
        }
        // A delayed provision can have finished after deletion was recorded.
        // Let runtime reconciliation remove it before requesting disk release.
        if self
            .adapter
            .runtime_inventory()
            .await
            .map_err(provider_error)?
            .iter()
            .any(|runtime| runtime.sandbox_id == observed.request.sandbox_id)
        {
            return Ok(());
        }
        if let Some(pools) = self
            .adapter
            .native_pools_for_home(&observed.request)
            .map_err(provider_error)?
        {
            pools
                .cleanup_retained_home(observed)
                .await
                .map_err(provider_error)?;
        }
        homes
            .remove(&observed.request, Some(&observed.receipt))
            .await
            .map_err(provider_error)?;
        Ok(())
    }

    async fn reconcile_native_claims(
        &self,
        stopping: &impl Fn() -> bool,
    ) -> Result<usize, SandboxStoreError> {
        let Some(pools) = self.adapter.native_pools() else {
            return Ok(0);
        };
        let inventory = pools.inventory_claims().await.map_err(provider_error)?;
        let count = inventory.len();
        for claim in inventory {
            if stopping() {
                break;
            }
            self.reconcile_native_claim(&claim).await?;
        }
        Ok(count)
    }

    pub(super) async fn reconcile_native_claim(
        &self,
        claim: &layer3_adapter::agent_sandbox::NativePoolClaim,
    ) -> Result<(), SandboxStoreError> {
        let id = SandboxId(claim.request.home.sandbox_id.clone());
        if self
            .store
            .unowned_home_reservation(&id)
            .await?
            .is_some_and(|home| {
                !home.released
                    && home
                        .native_pool
                        .as_ref()
                        .is_some_and(|native| !native.cleaned && native.request == claim.request)
            })
        {
            return Ok(());
        }
        let owner = self
            .adapter
            .native_pools_for_home(&claim.request.home)
            .map_err(provider_error)?
            .ok_or_else(|| corrupt("native claim belongs to another home authority"))?;
        if let Some(current) = self.store.get_sandbox(&id).await? {
            current
                .validate()
                .map_err(|error| corrupt(&error.to_string()))?;
            if current.lifecycle.home.as_ref().is_some_and(|home| {
                !home.released
                    && home
                        .native_pool
                        .as_ref()
                        .is_some_and(|native| native.request == claim.request)
            }) {
                return Ok(());
            }
        }
        owner.reclaim_claim(claim).await.map_err(provider_error)?;
        Ok(())
    }
}
