use kube::ResourceExt;

use super::{
    model, rejected, AgentSandboxPools, NativePoolClaim, NativePoolHomeOrigin, Result,
    NATIVE_HOME_ANNOTATION,
};
use crate::{provider::PersistentHomeVolumes, HomeVolumeAttachment};

impl AgentSandboxPools {
    /// Read-only recovery against durable claim authority. This remains possible
    /// after the claim and seed are deleted; it never adopts another claim's disk
    /// and is not proof that the original member/Pod has stopped using the home.
    pub async fn recover_home(
        &self,
        original: &NativePoolClaim,
    ) -> Result<Option<HomeVolumeAttachment>> {
        if original.request.home.policy.is_native_vct() {
            return self.recover_native_vct(original).await;
        }
        let desired = model::claim(&self.config, &original.request)?;
        if original.name != desired.name_any() || !super::super::model::dns_label(&original.uid) {
            return Err(rejected("native claim receipt is invalid"));
        }
        let Some(volume) = PersistentHomeVolumes::recover_native_claim(
            &self.volumes,
            &self.config.namespace,
            &self.config.deployment,
            &original.request.home,
        )
        .await?
        else {
            return Ok(None);
        };
        let origin: NativePoolHomeOrigin = serde_json::from_str(
            volume
                .annotations()
                .get(NATIVE_HOME_ANNOTATION)
                .ok_or_else(|| rejected("native home origin is missing"))?,
        )
        .map_err(|_| rejected("native home origin is invalid"))?;
        if origin.claim.request != original.request
            || origin.claim.name != original.name
            || origin.claim.uid != original.uid
            || original
                .sandbox_name
                .as_ref()
                .is_some_and(|name| origin.claim.sandbox_name.as_ref() != Some(name))
        {
            return Err(rejected(
                "native home does not belong to the recorded claim",
            ));
        }
        let receipt = PersistentHomeVolumes::native_home_receipt(
            &self.config.namespace,
            &self.config.deployment,
            &original.request.home,
            &volume,
        )?;
        Ok(Some(HomeVolumeAttachment {
            request: original.request.home.clone(),
            receipt,
        }))
    }
}
