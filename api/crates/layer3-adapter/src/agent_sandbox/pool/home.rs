//! Adopt the actual pre-created PVC without restarting the warm Pod. The claim
//! records its physical identities before any change to controller ownership.
use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::{
    api::{DynamicObject, Patch, PatchParams},
    ResourceExt,
};
use serde_json::{json, Value};

use super::{
    api_error, claims, model, rejected, unavailable, AgentSandboxPools, NativePoolClaim,
    NativePoolHomeOrigin, Result, API_VERSION, NATIVE_HOME_ANNOTATION, NATIVE_HOME_REQUEST_LABEL,
};
use crate::{provider::PersistentHomeVolumes, HomeVolumeAttachment};

impl AgentSandboxPools {
    /// The original claim receipt must already be durable. Repeated calls advance
    /// a claim-side adoption record; pending controller observation is retryable.
    /// Publish the returned attachment before using it to start an assigned runtime.
    pub async fn adopt_home(&self, original: &NativePoolClaim) -> Result<HomeVolumeAttachment> {
        self.guard_claim_ready(original).await?;
        if original.request.home.policy.is_native_vct() {
            return self.adopt_native_vct(original).await;
        }
        self.verify_explicit_template_policy(&original.request.pool)
            .await?;
        let desired_claim = model::claim(&self.config, &original.request)?;
        if original.name != desired_claim.name_any() {
            return Err(rejected("native claim receipt is invalid"));
        }
        let claim = self.claims.get(&original.name).await.map_err(api_error)?;
        claims::verify_claim(&claim, &desired_claim)?;
        if claim.uid().as_deref() != Some(&original.uid) {
            return Err(rejected("native claim UID changed"));
        }
        let current = claims::receipt(&original.request, &claim)?;
        if original.sandbox_name.is_some() && current.sandbox_name != original.sandbox_name {
            return Err(rejected("native claim changed its assigned Sandbox"));
        }
        let name = current
            .sandbox_name
            .as_deref()
            .ok_or_else(|| unavailable("native claim has no assigned Sandbox"))?;
        self.verify_seed(&original.request.home.policy).await?;
        let member = self.members.get(name).await.map_err(api_error)?;
        self.member_owned(&member, &current)?;
        let volume = self
            .volumes
            .get(&format!("home-{name}"))
            .await
            .map_err(api_error)?;
        let origin = if let Some(encoded) = claim.annotations().get(NATIVE_HOME_ANNOTATION) {
            let origin: NativePoolHomeOrigin = serde_json::from_str(encoded)
                .map_err(|_| rejected("native home adoption record is invalid"))?;
            origin.validate_for(
                &self.config.namespace,
                &self.config.deployment,
                &original.request.home,
            )?;
            if origin.claim.request != current.request
                || origin.claim.name != current.name
                || origin.claim.uid != current.uid
                || origin.claim.sandbox_name != current.sandbox_name
            {
                return Err(rejected("native home adoption authority changed"));
            }
            origin
        } else {
            let origin = NativePoolHomeOrigin {
                claim: current.clone(),
                sandbox_uid: member
                    .uid()
                    .ok_or_else(|| rejected("native Sandbox UID missing"))?,
                pvc_uid: volume
                    .uid()
                    .ok_or_else(|| rejected("native pool PVC UID missing"))?,
            };
            origin.validate_for(
                &self.config.namespace,
                &self.config.deployment,
                &original.request.home,
            )?;
            self.member_spec(&member, &origin, false)?;
            self.volume_candidate(&volume, &origin, false)?;
            let patch = json!({"metadata":{"uid":claim.uid(),"resourceVersion":claim.resource_version(),
                "annotations":{NATIVE_HOME_ANNOTATION:encode(&origin)?}}});
            // Losing this acknowledgement leaves a recoverable claim-side record.
            // Do not proceed to either physical mutation until it is acknowledged.
            let updated = self
                .claims
                .patch(
                    &claim.name_any(),
                    &PatchParams::default(),
                    &Patch::Merge(patch),
                )
                .await
                .map_err(api_error)?;
            if updated.uid() != claim.uid()
                || updated.annotations().get(NATIVE_HOME_ANNOTATION) != Some(&encode(&origin)?)
            {
                return Err(rejected("native home adoption record was not retained"));
            }
            origin
        };
        if member.uid().as_deref() != Some(&origin.sandbox_uid) {
            return Err(rejected("native pool Sandbox was replaced"));
        }
        self.volume_candidate(&volume, &origin, true)?;
        let converted = self.member_spec(&member, &origin, true)?;
        if !converted {
            let patch = json!({"metadata":{"uid":member.uid(),"resourceVersion":member.resource_version(),
                    "annotations":{NATIVE_HOME_ANNOTATION:encode(&origin)?}},
                "spec":{"volumeClaimTemplates":[],"podTemplate":{"spec":{"volumes":explicit_volumes(&origin)?}}}});
            self.members
                .patch(name, &PatchParams::default(), &Patch::Merge(patch))
                .await
                .map_err(api_error)?;
            return Err(unavailable(
                "native home is waiting for controller observation",
            ));
        }
        // Merely deleting a PVC ownerRef is unsafe: an older controller pass
        // reattaches it. Wait until that controller has observed the VCT removal.
        if !super::super::model::ready(&member) {
            return Err(unavailable(
                "native home is waiting for controller observation",
            ));
        }
        let attached = volume
            .metadata
            .owner_references
            .as_ref()
            .is_some_and(|refs| !refs.is_empty());
        let retained = if attached {
            let request = &original.request.home;
            let patch = json!({"metadata":{"uid":volume.uid(),"resourceVersion":volume.resource_version(),
                "ownerReferences":[],"labels":{"cma/home-managed-by":self.config.deployment,
                    NATIVE_HOME_REQUEST_LABEL:&request.computed_sha256()[..52]},
                "annotations":{"cma/home-allocation":serde_json::to_string(request).map_err(|_|rejected("invalid home allocation"))?,
                    "cma/home-request-sha256":request.computed_sha256(),NATIVE_HOME_ANNOTATION:encode(&origin)?}}});
            self.volumes
                .patch(
                    &origin.storage_reference()?,
                    &PatchParams::default(),
                    &Patch::Merge(patch),
                )
                .await
                .map_err(api_error)?
        } else {
            volume
        };
        let receipt = PersistentHomeVolumes::native_home_receipt(
            &self.config.namespace,
            &self.config.deployment,
            &original.request.home,
            &retained,
        )?;
        if receipt.storage_generation != origin.pvc_uid {
            return Err(rejected("native pool PVC was replaced during adoption"));
        }
        let after = self.members.get(name).await.map_err(api_error)?;
        self.member_owned(&after, &current)?;
        if !self.member_spec(&after, &origin, true)? || !super::super::model::ready(&after) {
            return Err(unavailable("native pool changed during home adoption"));
        }
        Ok(HomeVolumeAttachment {
            request: original.request.home.clone(),
            receipt,
        })
    }

    pub(crate) fn member_owned(
        &self,
        member: &DynamicObject,
        claim: &NativePoolClaim,
    ) -> Result<()> {
        let references = member
            .metadata
            .owner_references
            .as_deref()
            .unwrap_or_default();
        if member.namespace().as_deref() != Some(&self.config.namespace)
            || member.metadata.deletion_timestamp.is_some()
            || member.uid().as_deref().is_none_or(str::is_empty)
            || member
                .resource_version()
                .as_deref()
                .is_none_or(str::is_empty)
            || member.name_any() != claim.sandbox_name.as_deref().unwrap_or_default()
            || references.len() != 1
            || !references.iter().all(|owner| {
                owner.api_version == API_VERSION
                    && owner.kind == "SandboxClaim"
                    && owner.name == claim.name
                    && owner.uid == claim.uid
                    && owner.controller == Some(true)
            })
        {
            return Err(rejected(
                "native pool member is not owned by the admitted claim",
            ));
        }
        Ok(())
    }

    pub(crate) fn member_spec(
        &self,
        member: &DynamicObject,
        origin: &NativePoolHomeOrigin,
        allow_converted: bool,
    ) -> Result<bool> {
        if member.uid().as_deref() != Some(&origin.sandbox_uid) {
            return Err(rejected("native Sandbox UID changed"));
        }
        let original_config = model::binding_config(&self.config, &origin.claim.request.pool);
        let template = model::template(&original_config, &origin.claim.request.pool.spec)?;
        let mut desired = model::sandbox_spec(&template, &origin.claim.request.pool);
        if let Some(proof) =
            super::runtime_upgrade::RuntimeUpgradeProof::from_member(member, origin)?
        {
            super::runtime_upgrade::replace_image(&mut desired, proof.image())?;
        }
        if model::contains(&member.data["spec"], &desired) {
            if member.data["spec"]["podTemplate"]["spec"]
                .get("volumes")
                .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
            {
                return Err(rejected("native pool member has unadmitted volumes"));
            }
            if origin.claim.request.home.policy.is_native_vct() {
                return Ok(
                    member.annotations().get(NATIVE_HOME_ANNOTATION) == Some(&encode(origin)?)
                );
            }
            return Ok(false);
        }
        if origin.claim.request.home.policy.is_native_vct() {
            // A retained, unconverted home can be held for spec drift. A
            // changed VCT layout or home annotation is an authority refusal.
            if member.annotations().get(NATIVE_HOME_ANNOTATION) == Some(&encode(origin)?)
                && model::contains(
                    &member.data["spec"]["volumeClaimTemplates"],
                    &desired["volumeClaimTemplates"],
                )
                && member.data["spec"]["podTemplate"]["spec"]
                    .get("volumes")
                    .is_none_or(|volumes| volumes.as_array().is_some_and(Vec::is_empty))
            {
                return Err(crate::AdapterError::AdmittedSandboxSpecDrift);
            }
            return Err(rejected("native VCT ownership cannot be converted"));
        }
        desired["volumeClaimTemplates"] = json!([]);
        desired["podTemplate"]["spec"]["volumes"] = explicit_volumes(origin)?;
        if allow_converted
            && model::contains(&member.data["spec"], &desired)
            && member.annotations().get(NATIVE_HOME_ANNOTATION) == Some(&encode(origin)?)
        {
            Ok(true)
        } else {
            Err(rejected(
                "native pool member differs from its immutable template",
            ))
        }
    }

    pub(super) fn volume_candidate(
        &self,
        volume: &PersistentVolumeClaim,
        origin: &NativePoolHomeOrigin,
        allow_retained: bool,
    ) -> Result<()> {
        let request = &origin.claim.request.home;
        if request.policy.is_native_vct() {
            return self.validate_native_volume(volume, origin);
        }
        PersistentHomeVolumes::validate_pool_storage(
            &self.config.namespace,
            &self.config.deployment,
            request,
            volume,
        )?;
        if volume.name_any() != origin.storage_reference()?
            || volume.uid().as_deref() != Some(&origin.pvc_uid)
            || volume
                .resource_version()
                .as_deref()
                .is_none_or(str::is_empty)
            || volume.metadata.deletion_timestamp.is_some()
            || volume.annotations().get("cma/pool-seed-uid")
                != Some(&request.policy.template_generation)
            || volume.labels().get("cma/pool-config").map(String::as_str)
                != Some(&origin.claim.request.pool.config_sha256[..52])
        {
            return Err(rejected(
                "native pool PVC differs from its recorded lineage",
            ));
        }
        let owners = volume
            .metadata
            .owner_references
            .as_deref()
            .unwrap_or_default();
        if allow_retained && owners.is_empty() {
            let receipt = PersistentHomeVolumes::native_home_receipt(
                &self.config.namespace,
                &self.config.deployment,
                request,
                volume,
            )?;
            if receipt.storage_generation != origin.pvc_uid
                || volume.annotations().get(NATIVE_HOME_ANNOTATION) != Some(&encode(origin)?)
            {
                return Err(rejected("retained native home has another adoption record"));
            }
        } else if owners.len() != 1
            || !owners.iter().all(|owner| {
                owner.api_version == "agents.x-k8s.io/v1beta1"
                    && owner.kind == "Sandbox"
                    && Some(owner.name.as_str()) == origin.claim.sandbox_name.as_deref()
                    && owner.uid == origin.sandbox_uid
                    && owner.controller == Some(true)
            })
            || volume.annotations().contains_key("cma/home-allocation")
            || volume.labels().contains_key("cma/home-managed-by")
        {
            return Err(rejected(
                "native pool PVC ownership conflicts with adoption",
            ));
        }
        Ok(())
    }
}

pub(super) fn explicit_volumes(origin: &NativePoolHomeOrigin) -> Result<Value> {
    Ok(
        json!([{"name":"home","persistentVolumeClaim":{"claimName":origin.storage_reference()?,"readOnly":false}}]),
    )
}

fn encode(origin: &NativePoolHomeOrigin) -> Result<String> {
    serde_json::to_string(origin).map_err(|_| rejected("invalid native home adoption record"))
}
