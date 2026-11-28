//! Complete absence evidence for spare capacity; user resources are never deleted.
use super::{
    cleanup_inventory::complete, rejected, runtime_upgrade::RuntimeUpgradeProof, AgentSandboxPools,
    NativePoolBinding, NativePoolClaim, NativePoolHomeOrigin, Result,
};
use crate::HomeVolumeAttachment;
use kube::{api::DynamicObject, ResourceExt};
use std::collections::{BTreeMap, BTreeSet};

impl AgentSandboxPools {
    pub(super) async fn prove_retirement_homes(
        &self,
        binding: &NativePoolBinding,
        homes: &[HomeVolumeAttachment],
    ) -> Result<BTreeMap<String, DynamicObject>> {
        if homes.len() > 10_000 {
            return Err(rejected("native retirement homes exceed their bound"));
        }
        let mut members = BTreeMap::new();
        let mut claims = BTreeSet::new();
        let mut volumes = BTreeSet::new();
        for home in homes {
            let origin = origin(home)?;
            if origin.claim.request.pool != *binding
                || !claims.insert(origin.claim.uid.clone())
                || !volumes.insert(origin.pvc_uid.clone())
            {
                return Err(rejected(
                    "native retirement home is duplicated or belongs to another pool",
                ));
            }
            let member = self.prove_native_home(home).await?;
            if !self.retained_upgrade_policy(&member, origin)?
                || RuntimeUpgradeProof::from_member(&member, origin)?
                    .is_none_or(|proof| proof.image() == binding.spec.image.uri)
            {
                return Err(rejected(
                    "native retirement requires upgraded homes without template networking",
                ));
            }
            if members.insert(origin.sandbox_uid.clone(), member).is_some() {
                return Err(rejected("native retirement repeats a retained Sandbox"));
            }
        }
        Ok(members)
    }

    /// Shared by guarded draining and final retirement. A marker does not stop
    /// an already-created Claim from consuming spare capacity.
    pub(super) fn verify_retained_claim_inventory(
        &self,
        binding: &NativePoolBinding,
        expected_claims: &[&NativePoolClaim],
        claims: &[DynamicObject],
    ) -> Result<()> {
        let mut seen_claims = BTreeSet::new();
        for claim in claims {
            let recorded_pool = recorded_claim_pool(claim, binding, &self.config.deployment)?;
            let expected = expected_claims
                .iter()
                .find(|o| claim.uid().as_deref() == Some(&o.uid) || claim.name_any() == o.name);
            if let Some(expected) = expected {
                super::claims::verify_claim(
                    claim,
                    &super::model::claim(&self.config, &expected.request)?,
                )?;
                if claim.uid().as_deref() != Some(&expected.uid)
                    || super::claims::receipt(&expected.request, claim)?.sandbox_name
                        != expected.sandbox_name
                {
                    return Err(rejected("native retirement retained Claim changed"));
                }
                seen_claims.insert(expected.uid.clone());
            } else if claim
                .data
                .pointer("/spec/warmPoolRef/name")
                .and_then(|v| v.as_str())
                == Some(&binding.name)
                || claim
                    .data
                    .pointer("/spec/sandboxTemplateRef/name")
                    .and_then(|v| v.as_str())
                    == Some(&binding.template_name)
                || associated(claim, binding)
                || recorded_pool
            {
                return Err(rejected(
                    "native retirement found an unrecorded or pending Claim",
                ));
            }
        }
        if seen_claims.len() != expected_claims.len() {
            return Err(rejected(
                "native retirement inventory lost a retained Claim",
            ));
        }
        Ok(())
    }

    pub(super) async fn retirement_inventory(
        &self,
        binding: &NativePoolBinding,
        homes: &[HomeVolumeAttachment],
    ) -> Result<bool> {
        let retained = self.prove_retirement_homes(binding, homes).await?;
        let (claims, members, pods, volumes, pools) = tokio::join!(
            complete(&self.claims),
            complete(&self.members),
            complete(&self.pods),
            complete(&self.volumes),
            complete(&self.pools)
        );
        let claims = claims?;
        let members = members?;
        let pods = pods?;
        let volumes = volumes?;
        let pools = pools?;
        let origins = homes.iter().map(origin).collect::<Result<Vec<_>>>()?;
        let expected_claims = origins.iter().map(|o| &o.claim).collect::<Vec<_>>();
        self.verify_retained_claim_inventory(binding, &expected_claims, &claims)?;
        for pool in pools {
            if pool.name_any() == binding.name {
                if pool.uid().as_deref() != Some(&binding.uid) {
                    return Err(rejected("native retirement pool was replaced"));
                }
            } else if pool
                .data
                .pointer("/spec/sandboxTemplateRef/name")
                .and_then(|v| v.as_str())
                == Some(&binding.template_name)
            {
                return Err(rejected("native retirement template has another Pool"));
            }
        }
        let mut seen_members = BTreeSet::new();
        for member in &members {
            if let Some(expected) = member.uid().and_then(|uid| retained.get(&uid)) {
                if member.name_any() != expected.name_any()
                    || member.resource_version() != expected.resource_version()
                {
                    return Err(rejected(
                        "native retirement retained Sandbox changed during inventory",
                    ));
                }
                seen_members.insert(member.uid().unwrap());
            } else if associated(member, binding)
                || owners(member).iter().any(|owner| {
                    origins
                        .iter()
                        .any(|o| o.claim.uid == owner.uid || o.claim.name == owner.name)
                })
                || retained.values().any(|r| r.name_any() == member.name_any())
            {
                return Ok(false);
            }
        }
        if seen_members.len() != retained.len() {
            return Err(rejected(
                "native retirement inventory lost a retained Sandbox",
            ));
        }
        let mut seen_volumes = BTreeSet::new();
        for volume in &volumes {
            if let Some(expected) = origins.iter().find(|o| {
                volume.uid().as_deref() == Some(&o.pvc_uid)
                    || o.storage_reference().ok().as_deref() == Some(&volume.name_any())
            }) {
                self.validate_native_volume(volume, expected)?;
                seen_volumes.insert(expected.pvc_uid.clone());
            } else if associated(volume, binding)
                || dependent_capacity(volume, &members, &retained, binding)?
            {
                return Ok(false);
            }
        }
        if seen_volumes.len() != origins.len() {
            return Err(rejected("native retirement inventory lost a retained PVC"));
        }
        for pod in &pods {
            let retained_parent = owners(pod).iter().any(|o| {
                retained.get(&o.uid).is_some_and(|member| {
                    member.name_any() == o.name
                        && o.kind == "Sandbox"
                        && o.api_version == "agents.x-k8s.io/v1beta1"
                        && o.controller == Some(true)
                })
            });
            if retained_parent {
                if owners(pod).len() != 1 {
                    return Err(rejected(
                        "native retirement retained Pod has conflicting owners",
                    ));
                }
            } else if associated(pod, binding)
                || dependent_capacity(pod, &members, &retained, binding)?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn origin(home: &HomeVolumeAttachment) -> Result<&NativePoolHomeOrigin> {
    home.receipt
        .native_authority
        .as_deref()
        .ok_or_else(|| rejected("native retirement home authority is absent"))
}

fn recorded_claim_pool(
    claim: &DynamicObject,
    binding: &NativePoolBinding,
    deployment: &str,
) -> Result<bool> {
    let Some(encoded) = claim.annotations().get("cma/pool-claim-request") else {
        if claim
            .labels()
            .get(super::OWNER_LABEL)
            .is_some_and(|owner| owner == deployment)
        {
            return Err(rejected(
                "native retirement found a Claim without allocation authority",
            ));
        }
        return Ok(false);
    };
    if encoded.len() > 128 * 1024 {
        return Err(rejected(
            "native retirement Claim authority exceeds its bound",
        ));
    }
    let request: super::NativePoolClaimRequest = serde_json::from_str(encoded)
        .map_err(|_| rejected("native retirement Claim authority is invalid"))?;
    Ok(request.pool.uid == binding.uid
        || request.pool.name == binding.name
        || request.pool.template_name == binding.template_name
        || request.pool.template_uid == binding.template_uid)
}

pub(super) fn owners<K: kube::Resource>(
    resource: &K,
) -> &[k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference] {
    resource
        .meta()
        .owner_references
        .as_deref()
        .unwrap_or_default()
}

fn associated<K: kube::Resource>(resource: &K, binding: &NativePoolBinding) -> bool {
    let pool_hash = super::super::pods::name_hash(&binding.name);
    let template_hash = super::super::pods::name_hash(&binding.template_name);
    let labels = resource.labels();
    resource
        .name_any()
        .starts_with(&format!("{}-", binding.name))
        || resource
            .name_any()
            .starts_with(&format!("home-{}-", binding.name))
        || labels
            .get("cma/pool-config")
            .is_some_and(|s| s == &binding.config_sha256[..52])
        || labels
            .values()
            .any(|s| s == &pool_hash || s == &template_hash)
        || resource
            .annotations()
            .values()
            .any(|s| s == &binding.template_name || s == &binding.uid)
        || owners(resource).iter().any(|o| {
            o.uid == binding.uid
                || o.uid == binding.template_uid
                || o.name == binding.name
                || o.name == binding.template_name
        })
}

fn dependent_capacity<K: kube::Resource>(
    resource: &K,
    members: &[DynamicObject],
    retained: &BTreeMap<String, DynamicObject>,
    binding: &NativePoolBinding,
) -> Result<bool> {
    for owner in owners(resource) {
        if owner.kind != "Sandbox" || !owner.api_version.starts_with("agents.x-k8s.io/") {
            continue;
        }
        // Even when labels disappeared, an orphan can be the last spare's Pod or
        // PVC. Require its live original parent before excluding it as unrelated.
        let Some(member) = members
            .iter()
            .find(|m| m.uid().as_deref() == Some(&owner.uid) && m.name_any() == owner.name)
        else {
            return Ok(true);
        };
        if retained.contains_key(&owner.uid) {
            // Additional storage attached to a retained user is not spare proof.
            return Err(rejected(
                "native retirement found an unrecorded retained dependent",
            ));
        }
        if associated(member, binding) {
            return Ok(true);
        }
    }
    Ok(false)
}
