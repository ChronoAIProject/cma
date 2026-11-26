//! Drain only spare capacity; retained runtime upgrades govern later deletion.
use super::{
    api_error, cleanup_inventory::complete, management, rejected, retirement::RETIRING,
    unavailable, AgentSandboxPools, NativePoolBinding, NativePoolClaim, Result,
};
use crate::HomeVolumeAttachment;
use kube::{
    api::{DynamicObject, Patch, PatchParams},
    Resource, ResourceExt,
};
use serde_json::json;
use std::collections::BTreeSet;

const SPARE_DRAIN: &str = "cma/native-pool-spare-drain-v1";

impl AgentSandboxPools {
    /// Check drain evidence for an original, already-zero pool. Positive pools
    /// await a controller that deletes members with identity preconditions.
    pub async fn drain_pool(
        &self,
        binding: &NativePoolBinding,
        homes: &[HomeVolumeAttachment],
        adopted_claims: &[NativePoolClaim],
    ) -> Result<()> {
        let Some(pool) = self
            .checked_drain_pool(binding, homes, adopted_claims)
            .await?
        else {
            return Ok(());
        };
        // The WarmPool controller selects members from its cache and deletes
        // them by name. A pool CAS cannot fence those member deletes.
        if management::target(&pool)? != 0 {
            return Err(crate::AdapterError::Unsupported(
                "native pool drain requires UID and resourceVersion preconditions on controller member deletes".into(),
            ));
        }
        Ok(())
    }

    /// Leave replica changes to the provider that implements guarded member
    /// deletion. A legacy controller ignores this request and stays pending.
    pub(super) async fn request_spare_drain(
        &self,
        binding: &NativePoolBinding,
        homes: &[HomeVolumeAttachment],
        adopted_claims: &[NativePoolClaim],
    ) -> Result<()> {
        let Some(pool) = self
            .checked_drain_pool(binding, homes, adopted_claims)
            .await?
        else {
            return Ok(());
        };
        if let Some(request) = pool.annotations().get(SPARE_DRAIN) {
            if request != &binding.uid {
                return Err(rejected("native spare drain request has another pool UID"));
            }
        } else if management::target(&pool)? == 0 {
            return Ok(());
        } else {
            let version = pool
                .resource_version()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| rejected("native spare drain pool has no resource version"))?;
            let patch = json!({"metadata":{
                "uid":binding.uid,"resourceVersion":version,"annotations":{SPARE_DRAIN:binding.uid}
            }});
            let result = self
                .pools
                .patch(&binding.name, &PatchParams::default(), &Patch::Merge(patch))
                .await;
            let (_, current) = self.retirement_definitions(binding).await?;
            if !current.as_ref().is_some_and(|pool| {
                marked(pool).is_ok() && pool.annotations().get(SPARE_DRAIN) == Some(&binding.uid)
            }) {
                result.map_err(api_error)?;
                return Err(rejected("native spare drain request was not retained"));
            }
        }
        let state = management::status(binding.clone(), &pool)?;
        if state.desired_replicas != 0 || !state.observed || state.ready_replicas != 0 {
            return Err(unavailable(
                "native spare drain is pending provider completion",
            ));
        }
        Ok(())
    }

    async fn checked_drain_pool(
        &self,
        binding: &NativePoolBinding,
        homes: &[HomeVolumeAttachment],
        adopted_claims: &[NativePoolClaim],
    ) -> Result<Option<DynamicObject>> {
        let (_, pool) = self.retirement_definitions(binding).await?;
        let Some(pool) = pool else { return Ok(None) };
        marked(&pool)?;
        self.drain_inventory(binding, homes, adopted_claims).await?;
        let (_, current) = self.retirement_definitions(binding).await?;
        if let Some(pool) = &current {
            marked(pool)?;
        }
        Ok(current)
    }

    async fn drain_inventory(
        &self,
        binding: &NativePoolBinding,
        homes: &[HomeVolumeAttachment],
        adopted_claims: &[NativePoolClaim],
    ) -> Result<()> {
        if homes.len().saturating_add(adopted_claims.len()) > 10_000 {
            return Err(rejected("native drain homes exceed their bound"));
        }
        let mut identities = BTreeSet::new();
        let mut origins = Vec::new();
        for home in homes {
            home.validate_for(&home.request.sandbox_id)?;
            let origin = home
                .receipt
                .native_authority
                .as_deref()
                .ok_or_else(|| rejected("native drain home authority is absent"))?;
            origin.validate_for(
                &self.config.namespace,
                &self.config.deployment,
                &home.request,
            )?;
            if !home.request.policy.is_native_vct()
                || origin.claim.request.pool != *binding
                || !identities.insert(("claim", origin.claim.uid.clone()))
                || !identities.insert(("claim-name", origin.claim.name.clone()))
                || !identities.insert(("sandbox", origin.sandbox_uid.clone()))
                || !identities.insert(("sandbox-name", origin.claim.sandbox_name.clone().unwrap()))
                || !identities.insert(("pvc", origin.pvc_uid.clone()))
                || !identities.insert(("pvc-name", origin.storage_reference()?))
            {
                return Err(rejected(
                    "native drain home is duplicated or belongs to another pool",
                ));
            }
            origins.push(origin);
        }
        let (claims, members, volumes, pods) = tokio::join!(
            complete(&self.claims),
            complete(&self.members),
            complete(&self.volumes),
            complete(&self.pods),
        );
        let (claims, members, volumes, pods) = (claims?, members?, volumes?, pods?);
        let mut retained = Vec::new();
        for (home, origin) in homes.iter().zip(origins) {
            let name = origin.claim.sandbox_name.as_deref().unwrap();
            let storage = origin.storage_reference()?;
            let claim = original(&claims, &origin.claim.name, &origin.claim.uid)?;
            let member = original(&members, name, &origin.sandbox_uid)?;
            let volume = original(&volumes, &storage, &origin.pvc_uid)?;
            let present = match (claim, member, volume) {
                (Some(_), Some(member), Some(volume)) => {
                    let current = self.prove_native_home(home).await?;
                    if current.resource_version() != member.resource_version() {
                        return Err(rejected(
                            "native drain retained Sandbox changed during inventory",
                        ));
                    }
                    self.validate_native_volume(volume, origin)?;
                    retained.push(&origin.claim);
                    true
                }
                (None, None, None) => {
                    // Historical failed records remain in the database. Only
                    // complete absence of their original graph is exempted.
                    let (claim, member, volume) = tokio::join!(
                        self.claims.get_opt(&origin.claim.name),
                        self.members.get_opt(name),
                        self.volumes.get_opt(&storage),
                    );
                    if claim.map_err(api_error)?.is_some()
                        || member.map_err(api_error)?.is_some()
                        || volume.map_err(api_error)?.is_some()
                    {
                        return Err(rejected("native drain absent home reappeared"));
                    }
                    false
                }
                _ => return Err(rejected("native drain retained home is only partly absent")),
            };
            for member in &members {
                if depends_on(member, &origin.claim.name, &origin.claim.uid)
                    && (!present || member.uid().as_deref() != Some(&origin.sandbox_uid))
                {
                    return Err(rejected("native drain found an unrecorded Claim dependent"));
                }
            }
            for volume in &volumes {
                if depends_on(volume, name, &origin.sandbox_uid)
                    && (!present || volume.uid().as_deref() != Some(&origin.pvc_uid))
                {
                    return Err(rejected("native drain found an unrecorded retained PVC"));
                }
            }
            for pod in &pods {
                if depends_on(pod, name, &origin.sandbox_uid)
                    || pod.labels().get(super::super::pods::TRACKING_LABEL)
                        == Some(&super::super::pods::name_hash(name))
                {
                    let owners = pod.metadata.owner_references.as_deref().unwrap_or_default();
                    if !present
                        || owners.len() != 1
                        || owners.iter().any(|owner| {
                            owner.name != name
                                || owner.uid != origin.sandbox_uid
                                || owner.kind != "Sandbox"
                                || owner.api_version != "agents.x-k8s.io/v1beta1"
                                || owner.controller != Some(true)
                        })
                    {
                        return Err(rejected(
                            "native drain retained Pod authority is unresolved",
                        ));
                    }
                }
            }
        }
        let mut owned_members = retained
            .iter()
            .filter_map(|claim| claim.sandbox_name.clone())
            .collect::<BTreeSet<_>>();
        for expected in adopted_claims {
            let name = expected
                .sandbox_name
                .as_deref()
                .ok_or_else(|| rejected("native drain Claim has no persisted assignment"))?;
            let member = members
                .iter()
                .find(|member| member.name_any() == name)
                .ok_or_else(|| rejected("native drain Claim member is missing"))?;
            let claim = original(&claims, &expected.name, &expected.uid)?
                .ok_or_else(|| rejected("native drain original Claim is missing"))?;
            self.prove_drain_assignment(binding, expected, claim, member, &members)?;
            if !owned_members.insert(name.to_owned()) {
                return Err(rejected("native drain repeats a Claim member"));
            }
            retained.push(expected);
        }
        self.verify_retained_claim_inventory(binding, &retained, &claims)
    }
}

fn marked(pool: &DynamicObject) -> Result<()> {
    if pool.annotations().get(RETIRING).map(String::as_str) != Some("true")
        || pool.metadata.deletion_timestamp.is_some()
    {
        return Err(rejected(
            "native drain requires its original live retirement marker",
        ));
    }
    Ok(())
}

fn original<'a, K: Resource>(resources: &'a [K], name: &str, uid: &str) -> Result<Option<&'a K>> {
    let mut found = None;
    for resource in resources {
        if resource.name_any() == name || resource.uid().as_deref() == Some(uid) {
            if found.is_some()
                || resource.name_any() != name
                || resource.uid().as_deref() != Some(uid)
            {
                return Err(rejected("native drain original resource was replaced"));
            }
            found = Some(resource);
        }
    }
    Ok(found)
}

fn depends_on<K: Resource>(resource: &K, name: &str, uid: &str) -> bool {
    resource
        .meta()
        .owner_references
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|owner| owner.name == name || owner.uid == uid)
}
