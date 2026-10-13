//! CRD-owned homes retain the complete Claim → Sandbox → PVC UID chain.
use super::{
    api_error, model, rejected, unavailable, AgentSandboxPools, NativePoolClaim,
    NativePoolHomeOrigin, Result, NATIVE_HOME_ANNOTATION,
};
use crate::{HomeVolumeAttachment, HomeVolumeReceipt};
use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::{
    api::{Patch, PatchParams},
    ResourceExt,
};
use serde_json::json;

impl AgentSandboxPools {
    pub(super) async fn adopt_native_vct(
        &self,
        original: &NativePoolClaim,
    ) -> Result<HomeVolumeAttachment> {
        let current = self.observe_claim(original).await?;
        let name = current
            .sandbox_name
            .as_deref()
            .ok_or_else(|| unavailable("native claim has no assigned Sandbox"))?;
        let claim = self.claims.get(&current.name).await.map_err(api_error)?;
        if claim.uid().as_deref() != Some(&original.uid) {
            return Err(rejected("native claim UID changed"));
        }
        let member = self.members.get(name).await.map_err(api_error)?;
        self.member_owned(&member, &current)?;
        let volume = self
            .volumes
            .get(&format!("home-{name}"))
            .await
            .map_err(api_error)?;
        let origin = if let Some(encoded) = claim.annotations().get(NATIVE_HOME_ANNOTATION) {
            serde_json::from_str::<NativePoolHomeOrigin>(encoded)
                .map_err(|_| rejected("invalid native VCT authority"))?
        } else {
            NativePoolHomeOrigin {
                claim: current.clone(),
                sandbox_uid: member
                    .uid()
                    .ok_or_else(|| rejected("native Sandbox UID missing"))?,
                pvc_uid: volume
                    .uid()
                    .ok_or_else(|| rejected("native PVC UID missing"))?,
            }
        };
        origin.validate_for(
            &self.config.namespace,
            &self.config.deployment,
            &original.request.home,
        )?;
        if origin.claim.uid != current.uid
            || origin.claim.name != current.name
            || origin.claim.sandbox_name != current.sandbox_name
        {
            return Err(rejected("native VCT claim authority changed"));
        }
        self.member_spec(&member, &origin, false)?;
        self.validate_native_volume(&volume, &origin)?;
        let encoded =
            serde_json::to_string(&origin).map_err(|_| rejected("invalid native VCT authority"))?;
        if claim.annotations().get(NATIVE_HOME_ANNOTATION) != Some(&encoded) {
            let updated = self.claims.patch(&current.name, &PatchParams::default(), &Patch::Merge(json!({
                "metadata":{"uid":claim.uid(),"resourceVersion":claim.resource_version(),"annotations":{NATIVE_HOME_ANNOTATION:&encoded}}
            }))).await.map_err(api_error)?;
            if updated.uid() != claim.uid()
                || updated.annotations().get(NATIVE_HOME_ANNOTATION) != Some(&encoded)
            {
                return Err(rejected(
                    "native claim did not retain original home authority",
                ));
            }
        }
        if member.annotations().get(NATIVE_HOME_ANNOTATION) != Some(&encoded) {
            let updated = self.members.patch(name, &PatchParams::default(), &Patch::Merge(json!({
                "metadata":{"uid":member.uid(),"resourceVersion":member.resource_version(),"annotations":{NATIVE_HOME_ANNOTATION:&encoded}}
            }))).await.map_err(api_error)?;
            self.member_owned(&updated, &current)?;
            if updated.uid() != member.uid() || !self.member_spec(&updated, &origin, false)? {
                return Err(rejected(
                    "native Sandbox did not retain original home authority",
                ));
            }
        }
        // Acknowledgement proves initialization completed before credential admission.
        let latest = self.members.get(name).await.map_err(api_error)?;
        self.member_owned(&latest, &current)?;
        if !self.member_spec(&latest, &origin, false)? || !super::super::model::ready(&latest) {
            return Err(unavailable("native home initialization is still pending"));
        }
        let volume = self
            .volumes
            .get(&origin.storage_reference()?)
            .await
            .map_err(api_error)?;
        self.native_attachment(&origin, &volume)
    }

    pub(crate) fn native_attachment(
        &self,
        origin: &NativePoolHomeOrigin,
        volume: &PersistentVolumeClaim,
    ) -> Result<HomeVolumeAttachment> {
        origin.validate_for(
            &self.config.namespace,
            &self.config.deployment,
            &origin.claim.request.home,
        )?;
        self.validate_native_volume(volume, origin)?;
        let request = origin.claim.request.home.clone();
        let receipt = HomeVolumeReceipt {
            native_authority: Some(Box::new(origin.clone())),
            volume_id: request.volume_id.clone(),
            request_sha256: request.computed_sha256(),
            storage_reference: origin.storage_reference()?,
            storage_generation: origin.pvc_uid.clone(),
        };
        receipt.validate_for(&request)?;
        Ok(HomeVolumeAttachment { request, receipt })
    }

    pub(crate) fn validate_native_volume(
        &self,
        volume: &PersistentVolumeClaim,
        origin: &NativePoolHomeOrigin,
    ) -> Result<()> {
        let policy = &origin.claim.request.home.policy;
        policy.validate()?;
        let spec = volume
            .spec
            .as_ref()
            .ok_or_else(|| rejected("native PVC has no storage spec"))?;
        let owners = volume
            .metadata
            .owner_references
            .as_deref()
            .unwrap_or_default();
        let expected_resources = serde_json::from_value(
            json!({"requests":{"storage":format!("{}Gi",policy.capacity_gib)}}),
        )
        .map_err(|_| rejected("invalid native capacity"))?;
        if !policy.is_native_vct()
            || volume.namespace().as_deref() != Some(&self.config.namespace)
            || volume.name_any() != origin.storage_reference()?
            || volume.uid().as_deref() != Some(&origin.pvc_uid)
            || volume
                .resource_version()
                .as_deref()
                .is_none_or(str::is_empty)
            || volume.metadata.deletion_timestamp.is_some()
            || spec.storage_class_name.as_deref() != Some(&policy.storage_class)
            || spec.access_modes.as_deref() != Some(&["ReadWriteOncePod".into()])
            || spec
                .volume_mode
                .as_deref()
                .is_some_and(|v| v != "Filesystem")
            || spec.resources.as_ref() != Some(&expected_resources)
            || spec.data_source.is_some()
            || spec.data_source_ref.is_some()
            || spec.selector.is_some()
            || spec.volume_attributes_class_name.is_some()
            || volume.labels().get("cma/pool-config").map(String::as_str)
                != Some(&origin.claim.request.pool.config_sha256[..52])
            || volume
                .annotations()
                .get("cma/home-layout")
                .map(String::as_str)
                != Some("native-vct-empty-v1")
            || owners.len() != 1
            || !owners.iter().all(|o| {
                o.api_version == "agents.x-k8s.io/v1beta1"
                    && o.kind == "Sandbox"
                    && Some(o.name.as_str()) == origin.claim.sandbox_name.as_deref()
                    && o.uid == origin.sandbox_uid
                    && o.controller == Some(true)
            })
        {
            return Err(rejected(
                "native PVC differs from its admitted storage and controller authority",
            ));
        }
        Ok(())
    }

    pub(super) async fn recover_native_vct(
        &self,
        original: &NativePoolClaim,
    ) -> Result<Option<HomeVolumeAttachment>> {
        let desired = model::claim(&self.config, &original.request)?;
        if original.name != desired.name_any() {
            return Err(rejected("native claim receipt is invalid"));
        }
        let Some(claim) = self
            .claims
            .get_opt(&original.name)
            .await
            .map_err(api_error)?
        else {
            return Ok(None);
        };
        if claim.metadata.deletion_timestamp.is_some() {
            return Ok(None);
        }
        super::claims::verify_claim(&claim, &desired)?;
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
        let name = origin
            .claim
            .sandbox_name
            .as_deref()
            .ok_or_else(|| rejected("native Sandbox missing"))?;
        let Some(member) = self.members.get_opt(name).await.map_err(api_error)? else {
            return Ok(None);
        };
        if member.metadata.deletion_timestamp.is_some() {
            return Ok(None);
        }
        self.member_owned(&member, &origin.claim)?;
        if !self.member_spec(&member, &origin, false)? {
            return Ok(None);
        }
        let Some(volume) = self
            .volumes
            .get_opt(&origin.storage_reference()?)
            .await
            .map_err(api_error)?
        else {
            return Ok(None);
        };
        if volume.metadata.deletion_timestamp.is_some() {
            return Ok(None);
        }
        self.native_attachment(&origin, &volume).map(Some)
    }

    pub(crate) async fn volume_origin(
        &self,
        volume: &PersistentVolumeClaim,
    ) -> Result<Option<NativePoolHomeOrigin>> {
        if let Some(encoded) = volume.annotations().get(NATIVE_HOME_ANNOTATION) {
            return serde_json::from_str(encoded)
                .map(Some)
                .map_err(|_| rejected("invalid native home origin"));
        }
        let Some(owner) = volume
            .metadata
            .owner_references
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|o| {
                o.api_version == "agents.x-k8s.io/v1beta1"
                    && o.kind == "Sandbox"
                    && o.controller == Some(true)
            })
        else {
            return Ok(None);
        };
        let Some(member) = self.members.get_opt(&owner.name).await.map_err(api_error)? else {
            return Ok(None);
        };
        if member.uid().as_deref() != Some(&owner.uid) {
            return Err(rejected("native PVC owner was replaced"));
        }
        let Some(encoded) = member.annotations().get(NATIVE_HOME_ANNOTATION) else {
            return Ok(None);
        };
        let origin: NativePoolHomeOrigin =
            serde_json::from_str(encoded).map_err(|_| rejected("invalid native home origin"))?;
        self.native_attachment(&origin, volume)?;
        Ok(Some(origin))
    }
}
