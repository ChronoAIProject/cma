//! Cleanup also covers claims that never reached runtime admission. The caller
//! persists every newly discovered identity before allowing deletion effects.
use kube::{
    api::{DeleteParams, Preconditions, PropagationPolicy},
    ResourceExt,
};
use serde::{Deserialize, Serialize};

use super::{
    api_error, claims, model, rejected, unavailable, AgentSandboxPools, NativePoolClaim,
    NativePoolClaimRequest, Result,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolMember {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pvc_uid: Option<String>,
    pub name: String,
    pub uid: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolCleanup {
    pub request: NativePoolClaimRequest,
    pub claim: Option<NativePoolClaim>,
    pub members: Vec<NativePoolMember>,
}

impl NativePoolCleanup {
    pub fn validate(&self) -> Result<()> {
        self.request.validate()?;
        if self
            .claim
            .as_ref()
            .is_some_and(|claim| claim.request != self.request)
            || (self.claim.is_none() && !self.members.is_empty())
            || self.members.len() > 16
            || self.members.iter().any(|member| {
                !super::super::model::dns_label(&member.name)
                    || !super::super::model::dns_label(&member.uid)
                    || member
                        .pvc_uid
                        .as_ref()
                        .is_some_and(|v| !super::super::model::dns_label(v))
            })
        {
            return Err(rejected("native cleanup authority is invalid"));
        }
        let names: std::collections::BTreeSet<_> = self.members.iter().map(|m| &m.name).collect();
        if names.len() != self.members.len() {
            return Err(rejected("native cleanup repeats a member name"));
        }
        Ok(())
    }
}

impl AgentSandboxPools {
    /// Read-only recovery. Never recreate a claim to discover what to retire.
    pub async fn plan_cleanup(&self, original: &NativePoolCleanup) -> Result<NativePoolCleanup> {
        original.validate()?;
        let desired = model::claim(&self.config, &original.request)?;
        let mut next = original.clone();
        if let Some(recorded) = &original.claim {
            if recorded.name != desired.name_any() || !super::super::model::dns_label(&recorded.uid)
            {
                return Err(rejected("native cleanup has an invalid claim identity"));
            }
        }
        if let Some(mut actual) = self
            .claims
            .get_opt(&desired.name_any())
            .await
            .map_err(api_error)?
        {
            actual.metadata.deletion_timestamp = None;
            claims::verify_claim(&actual, &desired)?;
            let current = claims::receipt(&original.request, &actual)?;
            if let Some(recorded) = &original.claim {
                if current.uid != recorded.uid
                    || (recorded.sandbox_name.is_some()
                        && current.sandbox_name != recorded.sandbox_name)
                {
                    return Err(rejected("native cleanup claim identity changed"));
                }
                // A metadata version change does not grant new deletion authority.
                if recorded.sandbox_name.is_none() && current.sandbox_name.is_some() {
                    next.claim = Some(current);
                }
            } else {
                next.claim = Some(current);
            }
        }
        if let Some(claim) = &next.claim {
            for member in self.cleanup_members(claim).await? {
                if let Some(prior) = next.members.iter_mut().find(|v| v.name == member.name) {
                    if prior.uid != member.uid
                        || prior.pvc_uid.as_ref().is_some_and(|uid| {
                            member.pvc_uid.as_ref().is_some_and(|next| next != uid)
                        })
                    {
                        return Err(rejected("native cleanup member was replaced"));
                    }
                    if prior.pvc_uid.is_none() {
                        prior.pvc_uid = member.pvc_uid;
                    }
                } else {
                    next.members.push(member);
                }
            }
        }
        // A PVC can appear after the first plan, including after its Sandbox
        // disappeared. Capture its exact controller UID before cleanup effects.
        if next.request.home.policy.is_native_vct() {
            if let Some(claim) = &next.claim {
                for member in &mut next.members {
                    let Some(mut volume) = self
                        .volumes
                        .get_opt(&format!("home-{}", member.name))
                        .await
                        .map_err(api_error)?
                    else {
                        continue;
                    };
                    let uid = volume
                        .uid()
                        .ok_or_else(|| rejected("native PVC UID missing"))?;
                    if member.pvc_uid.as_ref().is_some_and(|prior| prior != &uid) {
                        return Err(rejected("native cleanup PVC was replaced"));
                    }
                    let mut original_claim = claim.clone();
                    original_claim.sandbox_name = Some(member.name.clone());
                    volume.metadata.deletion_timestamp = None;
                    self.validate_native_volume(
                        &volume,
                        &super::NativePoolHomeOrigin {
                            claim: original_claim,
                            sandbox_uid: member.uid.clone(),
                            pvc_uid: uid.clone(),
                        },
                    )?;
                    member.pvc_uid = Some(uid);
                }
            }
        }
        next.members.sort_by(|a, b| a.name.cmp(&b.name));
        next.validate()?;
        Ok(next)
    }

    /// Call only with a durable plan. Pending GC or an unrecorded member causes
    /// a retry; successful return proves claim, owned Sandbox and Pod absence.
    pub async fn cleanup(&self, original: &NativePoolCleanup) -> Result<()> {
        if self.plan_cleanup(original).await? != *original {
            return Err(unavailable(
                "native cleanup identities must be persisted first",
            ));
        }
        if let Some(claim) = &original.claim {
            self.delete_claim(claim).await?;
            for member in &original.members {
                let Some(actual) = self
                    .members
                    .get_opt(&member.name)
                    .await
                    .map_err(api_error)?
                else {
                    continue;
                };
                self.cleanup_member(&actual, claim)?;
                if actual.uid().as_deref() != Some(&member.uid) {
                    return Err(rejected("native cleanup member UID changed"));
                }
                if actual.metadata.deletion_timestamp.is_some() {
                    continue;
                }
                let params = DeleteParams {
                    propagation_policy: Some(PropagationPolicy::Foreground),
                    preconditions: Some(Preconditions {
                        uid: actual.uid(),
                        resource_version: actual.resource_version(),
                    }),
                    ..Default::default()
                };
                match self.members.delete(&member.name, &params).await {
                    Ok(_) => (),
                    Err(kube::Error::Api(error)) if error.code == 404 => (),
                    Err(error) => return Err(api_error(error)),
                }
            }
        }
        // Read again after mutations; a controller racing the initial snapshot
        // cannot be omitted from the next durable plan.
        if self.plan_cleanup(original).await? != *original {
            return Err(unavailable("native cleanup discovered another member"));
        }
        let desired = model::claim(&self.config, &original.request)?;
        if self
            .claims
            .get_opt(&desired.name_any())
            .await
            .map_err(api_error)?
            .is_some()
        {
            return Err(unavailable("native claim deletion remains pending"));
        }
        if let Some(claim) = &original.claim {
            if !self.cleanup_members(claim).await?.is_empty() {
                return Err(unavailable("native member deletion remains pending"));
            }
        }
        self.cleanup_pods_absent(original).await?;
        self.cleanup_owned_volumes(original).await
    }

    async fn cleanup_owned_volumes(&self, original: &NativePoolCleanup) -> Result<()> {
        let Some(claim) = &original.claim else {
            return Ok(());
        };
        for member in &original.members {
            let name = format!("home-{}", member.name);
            let Some(volume) = self.volumes.get_opt(&name).await.map_err(api_error)? else {
                continue;
            };
            if original.request.home.policy.is_native_vct() {
                if member.pvc_uid.as_deref() != volume.uid().as_deref() || member.pvc_uid.is_none()
                {
                    return Err(rejected(
                        "native cleanup PVC identity was not durably captured or changed",
                    ));
                }
                let mut assigned = claim.clone();
                assigned.sandbox_name = Some(member.name.clone());
                let origin = super::NativePoolHomeOrigin {
                    claim: assigned,
                    sandbox_uid: member.uid.clone(),
                    pvc_uid: member.pvc_uid.clone().unwrap(),
                };
                let mut observed = volume.clone();
                observed.metadata.deletion_timestamp = None;
                self.validate_native_volume(&observed, &origin)?;
                // The CRD controller/garbage collector owns deletion. Repeated
                // reads must observe absence before namespaced home finality.
                return Err(unavailable(
                    "controller-owned PVC garbage collection remains pending",
                ));
            }
            if volume
                .metadata
                .owner_references
                .as_ref()
                .is_none_or(Vec::is_empty)
            {
                // Retained disks are released by HomeVolumeStore using the
                // durable allocation receipt/native origin inventory.
                let origin: super::NativePoolHomeOrigin = volume
                    .annotations()
                    .get(super::NATIVE_HOME_ANNOTATION)
                    .and_then(|v| serde_json::from_str(v).ok())
                    .ok_or_else(|| rejected("native cleanup volume lost its origin"))?;
                origin.validate_for(
                    &self.config.namespace,
                    &self.config.deployment,
                    &original.request.home,
                )?;
                if origin.claim.uid != claim.uid
                    || origin.sandbox_uid != member.uid
                    || volume.uid().as_deref() != Some(&origin.pvc_uid)
                {
                    return Err(rejected("native cleanup volume changed identity"));
                }
                continue;
            }
            let mut assigned = claim.clone();
            assigned.sandbox_name = Some(member.name.clone());
            let origin = super::NativePoolHomeOrigin {
                claim: assigned,
                sandbox_uid: member.uid.clone(),
                pvc_uid: volume
                    .uid()
                    .ok_or_else(|| rejected("native cleanup volume has no UID"))?,
            };
            let mut observed = volume.clone();
            observed.metadata.deletion_timestamp = None;
            self.volume_candidate(&observed, &origin, false)?;
            if volume.metadata.deletion_timestamp.is_none() {
                let params = DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: volume.uid(),
                        resource_version: volume.resource_version(),
                    }),
                    ..Default::default()
                };
                match self.volumes.delete(&name, &params).await {
                    Ok(_) => (),
                    Err(kube::Error::Api(error)) if error.code == 404 => (),
                    Err(error) => return Err(api_error(error)),
                }
            }
            if self
                .volumes
                .get_opt(&name)
                .await
                .map_err(api_error)?
                .is_some()
            {
                return Err(unavailable("native member volume deletion remains pending"));
            }
        }
        Ok(())
    }
}
