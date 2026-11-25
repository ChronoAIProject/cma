//! CRD-owned homes retain the complete Claim → Sandbox → PVC UID chain.
use super::{
    api_error, model, rejected, unavailable, AgentSandboxPools, NativePoolClaim,
    NativePoolHomeOrigin, Result, NATIVE_HOME_ANNOTATION,
};
use crate::{HomeVolumeAttachment, HomeVolumeReceipt};
use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::{
    api::{DynamicObject, Patch, PatchParams},
    ResourceExt,
};
use serde_json::json;
use std::time::Duration;

struct NativeHomeRead {
    current: NativePoolClaim,
    claim: DynamicObject,
    member: DynamicObject,
    origin: NativePoolHomeOrigin,
}

impl AgentSandboxPools {
    pub(super) async fn adopt_native_vct(
        &self,
        original: &NativePoolClaim,
    ) -> Result<HomeVolumeAttachment> {
        // Only the read/validation portion may overlap policy. Both must finish
        // before the original ordered, UID/resourceVersion-fenced publications.
        let (policy, observed) = tokio::join!(
            self.verify_explicit_template_policy(&original.request.pool),
            self.read_native_adoption(original),
        );
        if policy
            .as_ref()
            .is_err_and(|error| error.is_native_pool_binding_hold())
        {
            // A configuration hold cannot take precedence over an independent
            // Claim/member/PVC authority failure.
            if let Err(error) = observed.as_ref() {
                return Err(error.clone());
            }
        }
        policy?;
        let NativeHomeRead {
            current,
            claim,
            member,
            origin,
        } = observed?;
        let name = current
            .sandbox_name
            .as_deref()
            .expect("validated assignment");
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
        // Publication acknowledgements remain outside this read-only deadline.
        // Start one phase before its first GET so a slow initial read cannot
        // escape the budget or grant subsequent polling a fresh two seconds.
        let readiness = super::super::NativeReadinessObservation::default();
        let observe = async {
            if let Some(home) = readiness.observe(self.observe_native_home(&origin)).await? {
                return Ok(home);
            }
            if self.config.native_claim_guard_enabled {
                self.wait_native_home(&origin, &readiness).await
            } else {
                tokio::time::timeout(
                    Duration::from_secs(5),
                    self.wait_native_home(&origin, &readiness),
                )
                .await
                .map_err(|_| readiness.timeout())?
            }
        };
        if self.config.native_claim_guard_enabled {
            super::super::claim_guard::bounded("home_authority", &readiness, observe).await
        } else {
            observe.await
        }
    }

    async fn read_native_adoption(&self, original: &NativePoolClaim) -> Result<NativeHomeRead> {
        let (current, claim) = self.observe_claim_resource(original).await?;
        let name = current
            .sandbox_name
            .as_deref()
            .ok_or_else(|| unavailable("native claim has no assigned Sandbox"))?;
        let volume_name = format!("home-{name}");
        // Only validated Claim assignment supplies these names. Retain this
        // Claim's resourceVersion for publication and member-before-PVC errors.
        let (member, volume) =
            tokio::join!(self.members.get(name), self.volumes.get(&volume_name),);
        let member = member.map_err(api_error)?;
        self.member_owned(&member, &current)?;
        let volume = volume.map_err(api_error)?;
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
        Ok(NativeHomeRead {
            current,
            claim,
            member,
            origin,
        })
    }

    async fn wait_native_home(
        &self,
        origin: &NativePoolHomeOrigin,
        readiness: &super::super::NativeReadinessObservation,
    ) -> Result<HomeVolumeAttachment> {
        // Only the original published authority may be observed here. Never
        // replay adoption writes or claim allocation while waiting for Ready.
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Some(home) = readiness.observe(self.observe_native_home(origin)).await? {
                return Ok(home);
            }
        }
    }

    async fn observe_native_home(
        &self,
        origin: &NativePoolHomeOrigin,
    ) -> Result<Option<HomeVolumeAttachment>> {
        let original = &origin.claim;
        let desired = model::claim(&self.config, &original.request)?;
        let name = original
            .sandbox_name
            .as_deref()
            .ok_or_else(|| rejected("native Sandbox missing"))?;
        let storage = origin.storage_reference()?;
        let (claim, member, volume) = tokio::join!(
            self.claims.get(&original.name),
            self.members.get(name),
            self.volumes.get(&storage),
        );
        let claim = claim.map_err(api_error)?;
        super::claims::verify_claim(&claim, &desired)?;
        if claim.uid().as_deref() != Some(&original.uid) {
            return Err(rejected("native claim UID changed"));
        }
        if super::claims::receipt(&original.request, &claim)?.sandbox_name != original.sandbox_name
        {
            return Err(rejected("native claim changed its assigned Sandbox"));
        }
        let encoded =
            serde_json::to_string(origin).map_err(|_| rejected("invalid native VCT authority"))?;
        if claim.annotations().get(NATIVE_HOME_ANNOTATION) != Some(&encoded) {
            return Err(rejected("native claim home authority changed"));
        }
        let member = member.map_err(api_error)?;
        self.member_owned(&member, original)?;
        if !self.member_spec(&member, origin, false)? {
            return Err(rejected("native Sandbox home authority changed"));
        }
        let attachment = self.native_attachment(origin, &volume.map_err(api_error)?)?;
        // Ready must acknowledge the annotation's current generation before
        // the returned attachment can admit runtime credentials.
        if super::super::model::ready(&member) {
            Ok(Some(attachment))
        } else {
            Ok(None)
        }
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
                != policy.native_layout()
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
