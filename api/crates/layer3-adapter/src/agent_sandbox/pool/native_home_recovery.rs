//! Recovery and verification share Claim → Sandbox → PVC validation order.
use super::{
    api_error, model, rejected, AgentSandboxPools, NativePoolClaim, NativePoolHomeOrigin, Result,
    NATIVE_HOME_ANNOTATION,
};
use crate::{AdapterError, HomeVolumeAttachment};
use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::{api::DynamicObject, ResourceExt};

impl AgentSandboxPools {
    pub(super) async fn recover_native_vct(
        &self,
        original: &NativePoolClaim,
    ) -> Result<Option<HomeVolumeAttachment>> {
        let desired = self.native_recovery_claim(original)?;
        let claim = self
            .claims
            .get_opt(&original.name)
            .await
            .map_err(api_error)?;
        let Some(origin) = recovery_origin(original, &desired, claim)? else {
            return Ok(None);
        };
        let name = origin
            .claim
            .sandbox_name
            .as_deref()
            .ok_or_else(|| rejected("native Sandbox missing"))?;
        let (member, volume) = tokio::join!(self.members.get_opt(name), async {
            self.volumes
                .get_opt(&origin.storage_reference()?)
                .await
                .map_err(api_error)
        });
        self.recovered_native_attachment(&origin, member, volume)
    }

    /// Verify an already validated attachment using its known resource names.
    /// The live Claim still supplies authority; the persisted origin only lets
    /// us issue the three independent reads before consuming their results.
    pub(super) async fn recover_known_native_vct(
        &self,
        recorded: &NativePoolHomeOrigin,
    ) -> Result<Option<HomeVolumeAttachment>> {
        let original = &recorded.claim;
        let desired = self.native_recovery_claim(original)?;
        let storage = recorded.storage_reference()?;
        let name = original
            .sandbox_name
            .as_deref()
            .ok_or_else(|| rejected("native Sandbox missing"))?;
        let (claim, member, volume) = tokio::join!(
            self.claims.get_opt(&original.name),
            self.members.get_opt(name),
            self.volumes.get_opt(&storage),
        );
        let Some(origin) = recovery_origin(original, &desired, claim.map_err(api_error)?)? else {
            return Ok(None);
        };
        if &origin != recorded {
            return Err(rejected(AdapterError::NATIVE_HOME_IDENTITY_REFUSAL));
        }
        // recovery_origin rejects a changed assigned name. PVC names derive
        // solely from that name, so these reads match the live origin exactly.
        self.recovered_native_attachment(&origin, member, volume.map_err(api_error))
    }

    pub(super) fn native_recovery_claim(
        &self,
        original: &NativePoolClaim,
    ) -> Result<DynamicObject> {
        let desired = model::claim(&self.config, &original.request)?;
        if original.name != desired.name_any() {
            return Err(rejected("native claim receipt is invalid"));
        }
        Ok(desired)
    }

    pub(super) fn recovered_native_attachment(
        &self,
        origin: &NativePoolHomeOrigin,
        member: std::result::Result<Option<DynamicObject>, kube::Error>,
        volume: Result<Option<PersistentVolumeClaim>>,
    ) -> Result<Option<HomeVolumeAttachment>> {
        let Some(member) = member.map_err(api_error)? else {
            return Ok(None);
        };
        if member.metadata.deletion_timestamp.is_some() {
            return Ok(None);
        }
        self.member_owned(&member, &origin.claim)?;
        let drifted = match self.member_spec(&member, origin, false) {
            Ok(true) => false,
            Ok(false) => return Ok(None),
            Err(AdapterError::AdmittedSandboxSpecDrift) => true,
            Err(error) => return Err(error),
        };
        let Some(volume) = volume? else {
            return Ok(None);
        };
        if volume.metadata.deletion_timestamp.is_some() {
            return Ok(None);
        }
        let attachment = self.native_attachment(origin, &volume)?;
        if drifted {
            return Err(AdapterError::AdmittedSandboxSpecDrift);
        }
        Ok(Some(attachment))
    }
}

pub(super) fn recovery_origin(
    original: &NativePoolClaim,
    desired: &DynamicObject,
    claim: Option<DynamicObject>,
) -> Result<Option<NativePoolHomeOrigin>> {
    let Some(claim) = claim else {
        return Ok(None);
    };
    if claim.metadata.deletion_timestamp.is_some() {
        return Ok(None);
    }
    super::claims::verify_claim(&claim, desired)?;
    if claim.uid().as_deref() != Some(&original.uid) {
        return Err(rejected("native claim UID changed"));
    }
    let Some(encoded) = claim.annotations().get(NATIVE_HOME_ANNOTATION) else {
        return Ok(None);
    };
    let origin: NativePoolHomeOrigin =
        serde_json::from_str(encoded).map_err(|_| rejected("invalid native VCT authority"))?;
    if origin.claim.request != original.request
        || origin.claim.uid != original.uid
        || original
            .sandbox_name
            .as_ref()
            .is_some_and(|name| origin.claim.sandbox_name.as_ref() != Some(name))
    {
        return Err(rejected("native home belongs to another claim"));
    }
    Ok(Some(origin))
}
