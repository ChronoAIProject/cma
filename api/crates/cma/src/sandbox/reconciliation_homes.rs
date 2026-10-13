//! Reclaim late allocations using fresh warm authority before owned authority.

use layer2_core::sandbox::SandboxStoreError;
use layer2_core::SandboxId;

use super::{corrupt, provider_error, SandboxReconciler};

impl SandboxReconciler {
    pub(super) async fn reconcile_homes(&self) -> Result<(), SandboxStoreError> {
        self.reconcile_native_claims().await?;
        let Some(homes) = self.adapter.home_volumes() else {
            return Ok(());
        };
        for observed in homes.inventory().await.map_err(provider_error)? {
            observed
                .validate_for(&observed.request.sandbox_id)
                .map_err(provider_error)?;
            if self.store.unowned_home_authority(&observed.request).await? {
                continue;
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
                    continue;
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
                continue;
            }
            if let Some(pools) = self
                .adapter
                .native_pools_for_home(&observed.request)
                .map_err(provider_error)?
            {
                pools
                    .cleanup_retained_home(&observed)
                    .await
                    .map_err(provider_error)?;
            }
            homes
                .remove(&observed.request, Some(&observed.receipt))
                .await
                .map_err(provider_error)?;
        }
        Ok(())
    }

    async fn reconcile_native_claims(&self) -> Result<(), SandboxStoreError> {
        let Some(pools) = self.adapter.native_pools() else {
            return Ok(());
        };
        for claim in pools.inventory_claims().await.map_err(provider_error)? {
            let owner = self
                .adapter
                .native_pools_for_home(&claim.request.home)
                .map_err(provider_error)?
                .ok_or_else(|| corrupt("native claim belongs to another home authority"))?;
            let id = SandboxId(claim.request.home.sandbox_id.clone());
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
                    continue;
                }
            }
            owner.reclaim_claim(&claim).await.map_err(provider_error)?;
        }
        Ok(())
    }
}
