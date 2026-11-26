//! Bounded image-transition evidence on the original native Sandbox only.
use super::{rejected, NativePoolHomeOrigin, Result};
use crate::{provider::ProviderSandboxRequest, provider_request_digest};
use kube::{api::DynamicObject, ResourceExt};
use serde::{Deserialize, Serialize};

pub(crate) const UPGRADE_ANNOTATION: &str = "cma/native-runtime-upgrade";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedStartupPolicyProof {
    pub operation_id: String,
    pub predecessor_resource_version: String,
    pub journal_recovery: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeUpgradeProof {
    version: u8,
    origin_sha256: String,
    previous_image: String,
    previous_generation: u64,
    previous_request_sha256: String,
    image: String,
    generation: u64,
    request_sha256: String,
    previous_proof_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retained_policy: Option<RetainedStartupPolicyProof>,
}

impl RuntimeUpgradeProof {
    pub(crate) fn create(
        origin: &NativePoolHomeOrigin,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        prior: Option<&Self>,
        retained_policy: Option<RetainedStartupPolicyProof>,
    ) -> Result<Self> {
        let proof = Self {
            version: if retained_policy.is_some() { 2 } else { 1 },
            origin_sha256: provider_request_digest(origin)?,
            previous_image: previous.image.uri.clone(),
            previous_generation: crate::hosted::provider_request_identity(previous)?.generation,
            previous_request_sha256: provider_request_digest(previous)?,
            image: next.image.uri.clone(),
            generation: crate::hosted::provider_request_identity(next)?.generation,
            request_sha256: provider_request_digest(next)?,
            previous_proof_sha256: prior.map(provider_request_digest).transpose()?,
            retained_policy,
        };
        proof.validate(origin)?;
        proof.validate_request(next)?;
        Ok(proof)
    }

    pub(crate) fn from_member(
        member: &DynamicObject,
        origin: &NativePoolHomeOrigin,
    ) -> Result<Option<Self>> {
        let Some(encoded) = member.annotations().get(UPGRADE_ANNOTATION) else {
            return Ok(None);
        };
        if encoded.len() > 8192 {
            return Err(rejected("native runtime upgrade proof exceeds its bound"));
        }
        let proof: Self = serde_json::from_str(encoded)
            .map_err(|_| rejected("native runtime upgrade proof is invalid"))?;
        proof.validate(origin)?;
        let generation = member
            .annotations()
            .get("cma/runtime-generation")
            .and_then(|g| g.parse::<u64>().ok())
            .ok_or_else(|| rejected("native runtime upgrade generation is absent"))?;
        if member.uid().as_deref() != Some(&origin.sandbox_uid)
            || generation < proof.generation
            || member.annotations().get("cma/image-sha256")
                != Some(&crate::hosted::image_sha256(&proof.image))
            || generation == proof.generation
                && member.annotations().get(super::RUNTIME_REQUEST_ANNOTATION)
                    != Some(&proof.request_sha256)
        {
            return Err(rejected(
                "native runtime upgrade proof differs from current authority",
            ));
        }
        Ok(Some(proof))
    }

    fn validate(&self, origin: &NativePoolHomeOrigin) -> Result<()> {
        let digest = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        };
        crate::hosted::validate_runtime_image(&self.image)?;
        crate::hosted::validate_runtime_image(&self.previous_image)?;
        if !matches!(
            (self.version, &self.retained_policy),
            (1, None) | (2, Some(_))
        ) || self.retained_policy.as_ref().is_some_and(|policy| {
            policy.operation_id.is_empty()
                || policy.operation_id.len() > 128
                || policy.operation_id.trim() != policy.operation_id
                || policy.operation_id.chars().any(char::is_control)
                || policy.predecessor_resource_version.is_empty()
        }) || !origin.claim.request.home.policy.is_native_vct()
            || self.origin_sha256 != provider_request_digest(origin)?
            || self.previous_generation < origin.claim.request.runtime_generation
            || self.generation <= self.previous_generation
            || !digest(&self.previous_request_sha256)
            || !digest(&self.request_sha256)
            || self
                .previous_proof_sha256
                .as_deref()
                .is_some_and(|s| !digest(s))
            || self.previous_proof_sha256.is_none()
                && self.previous_image != origin.claim.request.pool.spec.image.uri
        {
            return Err(rejected(
                "native runtime upgrade proof changed its original lineage",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_request(&self, request: &ProviderSandboxRequest) -> Result<()> {
        let generation = crate::hosted::provider_request_identity(request)?.generation;
        if request.image.uri != self.image
            || generation < self.generation
            || self.retained_policy.as_ref().is_some_and(|policy| {
                request
                    .env
                    .get(crate::hosted::JOURNAL_RECOVERY)
                    .map(String::as_str)
                    != Some(if policy.journal_recovery {
                        "true"
                    } else {
                        "false"
                    })
            })
            || generation == self.generation
                && provider_request_digest(request)? != self.request_sha256
        {
            return Err(rejected(
                "runtime request differs from its explicit image upgrade",
            ));
        }
        Ok(())
    }

    pub(crate) fn retained_policy(&self) -> Option<&RetainedStartupPolicyProof> {
        self.retained_policy.as_ref()
    }

    pub(crate) fn is_transition(
        &self,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
    ) -> Result<bool> {
        Ok(
            self.previous_request_sha256 == provider_request_digest(previous)?
                && self.request_sha256 == provider_request_digest(next)?,
        )
    }

    pub(crate) fn encode(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|_| rejected("invalid native runtime upgrade proof"))
    }

    pub(crate) fn image(&self) -> &str {
        &self.image
    }
}

/// Preserve the original template hashes, immutable VCT and all policy fields.
pub(crate) fn replace_image(spec: &mut serde_json::Value, image: &str) -> Result<()> {
    for (field, name) in [
        ("containers", "runtime"),
        ("initContainers", "initialize-home"),
    ] {
        let containers = spec["podTemplate"]["spec"][field]
            .as_array_mut()
            .ok_or_else(|| rejected("native runtime template has no image slot"))?;
        let container = containers
            .iter_mut()
            .find(|c| c["name"] == name)
            .ok_or_else(|| rejected("native runtime template image slot changed"))?;
        container["image"] = serde_json::json!(image);
    }
    Ok(())
}

impl super::AgentSandboxPools {
    /// An upgraded, still-owned native member carries its complete immutable Pod
    /// policy in the original home authority. Template-managed networking remains
    /// an external dependency and must never use this retirement exemption.
    pub(crate) fn retained_upgrade_policy(
        &self,
        member: &DynamicObject,
        origin: &NativePoolHomeOrigin,
    ) -> Result<bool> {
        if !origin.claim.request.home.policy.is_native_vct()
            || origin
                .claim
                .request
                .pool
                .pod_policy
                .template_network_policy
                .is_some()
            || RuntimeUpgradeProof::from_member(member, origin)?.is_none()
        {
            return Ok(false);
        }
        self.member_owned(member, &origin.claim)?;
        if !self.member_spec(member, origin, false)? {
            return Err(rejected(
                "upgraded native member lost its original home policy",
            ));
        }
        Ok(true)
    }
}
