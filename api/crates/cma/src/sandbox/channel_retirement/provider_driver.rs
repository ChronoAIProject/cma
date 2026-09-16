use super::*;
use crate::sandbox::channel_retirement::provider_finality::{
    self, ProviderFinalityReceipt, ProviderHomeFinality,
};
use crate::store::bridge_allocations::channel_retirement::provider_finality::ReadyRuntimeJournal;

impl ChannelRetirementDriver {
    pub(super) async fn provider_teardown(
        &self,
        permit: &ChannelRetirementAuthority,
        snapshot: &ChannelRetirementSnapshot,
        ready: ReadyRuntimeJournal,
        now: SystemTime,
    ) -> Result<RetirementStep, AllocationError> {
        let Some(runtime) = ready.runtime_finality else {
            crate::sandbox::runtime_attachment::adopt_bound_runtime(
                self.adapter.as_ref(),
                &snapshot.sandbox,
                &snapshot.provisioning.settings.deployment_id,
            )
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
            self.read(permit, now).await?;
            let receipt = self
                .adapter
                .finalize_runtime(
                    &ready.original,
                    &provider_finality::delete_operation_id(snapshot),
                )
                .await
                .map_err(|_| AllocationError::AuthorityUnavailable)?;
            return self
                .commit(permit, Change::ProviderRuntimeFinality(&receipt), now)
                .await;
        };
        let home = if let Some(original) = &ready.home {
            let homes = self
                .adapter
                .home_volumes()
                .ok_or(AllocationError::AuthorityUnavailable)?;
            let Some(delete) = ready.home_delete else {
                self.read(permit, now).await?;
                homes
                    .remove_captures(original.home())
                    .await
                    .map_err(|_| AllocationError::AuthorityUnavailable)?;
                self.read(permit, now).await?;
                let delete = homes
                    .delete_original_home(
                        original,
                        &provider_finality::home_delete_operation_id(snapshot),
                    )
                    .await
                    .map_err(|_| AllocationError::AuthorityUnavailable)?;
                return self
                    .commit(permit, Change::ProviderHomeDelete(&delete), now)
                    .await;
            };
            self.read(permit, now).await?;
            let receipt = homes
                .observe_home_finality(&delete)
                .await
                .map_err(|_| AllocationError::AuthorityUnavailable)?;
            ProviderHomeFinality::Removed {
                receipt: Box::new(receipt),
            }
        } else {
            ProviderHomeFinality::NotConfigured {
                original_settings_sha256: ready.origin.original_settings_sha256.clone(),
            }
        };
        let receipt = ProviderFinalityReceipt {
            schema_version: 1,
            origin: ready.origin,
            runtime_dispatch_set_sha256: ready.runtime_dispatch_set_sha256,
            home_dispatch_set_sha256: ready.home_dispatch_set_sha256,
            runtime,
            home,
        };
        receipt.require(snapshot)?;
        self.commit(permit, Change::ProviderFinality(&receipt), now)
            .await
    }
}
