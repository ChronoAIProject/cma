//! Bind logical runtime authority to an adopted pool member without replacing its Pod template.
use super::runtime_upgrade::{RuntimeUpgradeProof, UPGRADE_ANNOTATION};
use kube::api::DynamicObject;
use serde_json::json;

use super::{
    api_error, model, rejected, AgentSandboxPools, NativePoolHomeOrigin, Result,
    NATIVE_HOME_ANNOTATION, OWNER_LABEL,
};
use crate::{
    provider::{PersistentHomeVolumes, ProviderSandboxRequest},
    HomeVolumeAttachment,
};

pub(crate) const RUNTIME_REQUEST_ANNOTATION: &str = "cma/native-runtime-request-sha256";

impl AgentSandboxPools {
    pub(crate) async fn runtime_desired(
        &self,
        origin: &NativePoolHomeOrigin,
        request: &ProviderSandboxRequest,
        name: &str,
    ) -> Result<DynamicObject> {
        origin.validate_for(
            &self.config.namespace,
            &self.config.deployment,
            &origin.claim.request.home,
        )?;
        if origin.claim.sandbox_name.as_deref() != Some(name) {
            return Err(rejected(
                "native runtime must retain its original Sandbox name",
            ));
        }
        if origin.claim.request.home.policy.is_native_vct() {
            let storage = origin.storage_reference()?;
            // The validated persisted origin supplies all three names. Read
            // independently, but retain Claim, PVC, then member validation and
            // error precedence. This rendering proof never replaces the later
            // suspension observation or fresh transition CAS.
            let (claim, volume, member) = tokio::join!(
                self.observe_claim(&origin.claim),
                self.volumes.get(&storage),
                self.members.get(name),
            );
            claim?;
            let volume = volume.map_err(api_error)?;
            self.volume_candidate(&volume, origin, true)?;
            let home = self.native_attachment(origin, &volume)?;
            let member = member.map_err(api_error)?;
            self.member_owned(&member, &origin.claim)?;
            if !self.member_spec(&member, origin, false)? {
                return Err(rejected("native runtime home authority changed"));
            }
            return self.runtime_desired_for_member(origin, &home, request, name, &member);
        }
        // The claim must still own the admitted member; disappearance or a
        // replacement claim cannot authorize a different runtime incarnation.
        self.observe_claim(&origin.claim).await?;
        let volume = self
            .volumes
            .get(&origin.storage_reference()?)
            .await
            .map_err(api_error)?;
        self.volume_candidate(&volume, origin, true)?;
        let home = HomeVolumeAttachment {
            request: origin.claim.request.home.clone(),
            receipt: PersistentHomeVolumes::native_home_receipt(
                &self.config.namespace,
                &self.config.deployment,
                &origin.claim.request.home,
                &volume,
            )?,
        };
        self.runtime_desired_for_home(origin, &home, request, name)
    }

    /// Pure rendering after the caller has verified the exact current home graph.
    pub(crate) fn runtime_desired_for_home(
        &self,
        origin: &NativePoolHomeOrigin,
        home: &HomeVolumeAttachment,
        request: &ProviderSandboxRequest,
        name: &str,
    ) -> Result<DynamicObject> {
        self.runtime_desired_with_upgrade(origin, home, request, name, None)
    }

    pub(crate) fn runtime_desired_for_member(
        &self,
        origin: &NativePoolHomeOrigin,
        home: &HomeVolumeAttachment,
        request: &ProviderSandboxRequest,
        name: &str,
        member: &DynamicObject,
    ) -> Result<DynamicObject> {
        let proof = RuntimeUpgradeProof::from_member(member, origin)?;
        self.runtime_desired_with_upgrade(origin, home, request, name, proof.as_ref())
    }

    pub(crate) fn runtime_desired_with_upgrade(
        &self,
        origin: &NativePoolHomeOrigin,
        home: &HomeVolumeAttachment,
        request: &ProviderSandboxRequest,
        name: &str,
        proof: Option<&RuntimeUpgradeProof>,
    ) -> Result<DynamicObject> {
        let image = match proof {
            Some(proof) => {
                proof.validate_request(request)?;
                proof.image()
            }
            None => &origin.claim.request.pool.spec.image.uri,
        };
        validate_request(&self.config.deployment, origin, home, request, image)?;
        let config = model::binding_config(&self.config, &origin.claim.request.pool);
        let template = model::template(&config, &origin.claim.request.pool.spec)?;
        let mut spec = model::sandbox_spec(&template, &origin.claim.request.pool);
        if !origin.claim.request.home.policy.is_native_vct() {
            spec["volumeClaimTemplates"] = json!([]);
            spec["podTemplate"]["spec"]["volumes"] = super::home::explicit_volumes(origin)?;
        }
        if proof.is_some() {
            super::runtime_upgrade::replace_image(&mut spec, image)?;
        }
        spec["operatingMode"] = json!("Running");
        let mut annotations = request.metadata.clone();
        if let Some(proof) = proof {
            annotations.insert(UPGRADE_ANNOTATION.into(), proof.encode()?);
        }
        annotations.insert(
            NATIVE_HOME_ANNOTATION.into(),
            serde_json::to_string(origin).map_err(|_| rejected("invalid native home origin"))?,
        );
        annotations.insert(
            RUNTIME_REQUEST_ANNOTATION.into(),
            crate::provider_request_digest(request)?,
        );
        annotations.insert(
            super::super::SPEC_ANNOTATION.into(),
            crate::provider_request_digest(&spec)?,
        );
        super::super::funding::apply(&mut spec, request.funded_until)?;
        serde_json::from_value(json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
            "metadata":{"name":name,"namespace":self.config.namespace,
                "labels":{OWNER_LABEL:self.config.deployment},"annotations":annotations},"spec":spec}))
            .map_err(|_| rejected("invalid native runtime configuration"))
    }

    /// Validate persisted request correlation without observing live authority.
    pub(crate) fn validate_runtime_home_request(
        &self,
        origin: &NativePoolHomeOrigin,
        home: &HomeVolumeAttachment,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        validate_request(
            &self.config.deployment,
            origin,
            home,
            request,
            &origin.claim.request.pool.spec.image.uri,
        )
    }
}

fn validate_request(
    deployment: &str,
    origin: &NativePoolHomeOrigin,
    home: &HomeVolumeAttachment,
    request: &ProviderSandboxRequest,
    image: &str,
) -> Result<()> {
    let identity = crate::hosted::provider_request_identity(request)?;
    let mut unfingerprinted = request.clone();
    unfingerprinted.metadata.remove("cma/spec-sha256");
    let spec = &origin.claim.request.pool.spec;
    if identity.spec_fingerprint != crate::hosted::provider_request_fingerprint(&unfingerprinted)?
        || identity.sandbox_id != home.request.sandbox_id
        || identity.runtime_contract != crate::RUNTIME_CONTRACT_VERSION
        || identity.generation < origin.claim.request.runtime_generation
        || request.metadata.get(OWNER_LABEL).map(String::as_str) != Some(deployment)
        || request.metadata.get("cma/home-identity") != Some(&home.identity()?)
        || request.image.uri != image
        || request.entrypoint != [crate::DEFAULT_SUPERVISOR_PROGRAM]
        || request.timeout.is_some()
        || request.resource_limits
            != std::collections::BTreeMap::from([
                ("cpu".into(), format!("{}m", spec.cpu_millis)),
                ("memory".into(), format!("{}Mi", spec.memory_mib)),
            ])
        || request
            .metadata
            .keys()
            .any(|k| k.starts_with("cma/native-") || k == super::super::SPEC_ANNOTATION)
        || request.env.contains_key("CHRONO_RUNTIME_CONTROL_TOKEN")
        || request.env.get("CHRONO_RUNTIME_CONTRACT") != Some(&identity.runtime_contract)
        || request.env.get("CHRONO_SANDBOX_ID") != Some(&home.request.sandbox_id)
        || request.env.get("CHRONO_RUNTIME_GENERATION") != Some(&identity.generation.to_string())
        || request.env.get("CHRONO_RUNTIME_HOME_IDENTITY") != Some(&home.identity()?)
        || ["HOME", "CHRONO_RUNTIME_HOME"]
            .into_iter()
            .any(|k| request.env.get(k).map(String::as_str) != Some(crate::DEFAULT_HOME_DIRECTORY))
    {
        return Err(rejected(
            "runtime request differs from its admitted native pool/home",
        ));
    }
    let expected = [
        (
            "cma-home",
            home.request.policy.mount_path("/home/cma"),
            "home",
        ),
        ("cma-state", "/var/lib/cma-runtime", "state"),
        ("cma-transfer", "/var/lib/cma-transfer", "transfer"),
    ];
    request.validate_volumes()?;
    if request.volumes.len() != expected.len()
        || request
            .volumes
            .iter()
            .zip(expected)
            .any(|(v, (name, path, sub))| {
                v.name != name
                    || v.mount_path != path
                    || v.sub_path.as_deref() != Some(sub)
                    || v.read_only
                    || v.pvc.claim_name != home.receipt.storage_reference
            })
    {
        return Err(rejected(
            "native runtime mounts differ from its original pool home",
        ));
    }
    // Validate the complete assignment envelope before publishing any authority.
    // The real token and Pod UID are inserted only immediately before exec.
    let mut environment = request.env.clone();
    environment.insert("CHRONO_RUNTIME_CONTROL_TOKEN".into(), "0".repeat(64));
    crate::runtime_bootstrap::RuntimeBootstrapRequest {
        version: crate::runtime_bootstrap::RUNTIME_BOOTSTRAP_VERSION,
        pod_uid: "pending-pod".into(),
        staging_startup_failure: None,
        environment,
    }
    .validate()
}
