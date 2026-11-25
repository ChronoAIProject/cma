use super::{
    rejected, retirement_inventory::owners, AgentSandboxPools, NativePoolBinding, NativePoolClaim,
    Result, API_VERSION, NATIVE_HOME_ANNOTATION,
};
use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::{api::DynamicObject, Resource, ResourceExt};
use std::collections::BTreeSet;

const POOL_LABEL: &str = "agents.x-k8s.io/warm-pool-sandbox";
const CLAIM_LABEL: &str = "agents.x-k8s.io/claim-uid";
const ASSIGNMENT: &str = "agents.x-k8s.io/sandbox-name";
const SANDBOX_VERSION: &str = "agents.x-k8s.io/v1beta1";

impl AgentSandboxPools {
    pub(super) fn prove_drain_assignment(
        &self,
        binding: &NativePoolBinding,
        expected: &NativePoolClaim,
        claim: &DynamicObject,
        member: &DynamicObject,
        members: &[DynamicObject],
    ) -> Result<()> {
        let name = member.name_any();
        self.cleanup_member(member, expected)?;
        if expected.request.pool != *binding
            || member.metadata.deletion_timestamp.is_some()
            || member.labels().contains_key(POOL_LABEL)
            || member
                .data
                .pointer("/spec/podTemplate/metadata/labels/agents.x-k8s.io~1claim-uid")
                .and_then(|label| label.as_str())
                != Some(&expected.uid)
            || assignment_names(claim).any(|assigned| assigned != name)
            || members.iter().any(|other| {
                other.name_any() != name
                    && owners(other)
                        .iter()
                        .any(|owner| owner.uid == expected.uid || owner.name == expected.name)
            })
        {
            return Err(rejected("native drain Claim ownership is ambiguous"));
        }
        Ok(())
    }
}

pub(super) fn prove(
    binding: &NativePoolBinding,
    owned: &BTreeSet<String>,
    claims: &[DynamicObject],
    members: &[DynamicObject],
    pods: &[DynamicObject],
    volumes: &[PersistentVolumeClaim],
) -> Result<()> {
    use super::super::pods::{name_hash, TRACKING_LABEL};

    let pool_hash = name_hash(&binding.name);
    let assigned = claims
        .iter()
        .flat_map(assignment_names)
        .collect::<BTreeSet<_>>();
    let mut names = BTreeSet::new();
    for member in members {
        let in_pool = member.labels().get(POOL_LABEL) == Some(&pool_hash);
        let pool_owner = owners(member).iter().any(|owner| owner.uid == binding.uid);
        if !in_pool && !pool_owner {
            continue;
        }
        let name = member.name_any();
        if !in_pool
            || !names.insert(name.clone())
            || owned.contains(&name)
            || assigned.contains(name.as_str())
            || !sole_owner(
                member,
                API_VERSION,
                "SandboxWarmPool",
                &binding.name,
                &binding.uid,
            )
            || member.labels().contains_key(CLAIM_LABEL)
            || member.annotations().contains_key("cma/sandbox")
            || member.annotations().contains_key(NATIVE_HOME_ANNOTATION)
        {
            return Err(rejected("native drain found an ambiguous spare Sandbox"));
        }
        let uid = member
            .uid()
            .ok_or_else(|| rejected("native drain spare UID is missing"))?;
        let storage = format!("home-{name}");
        let hash = name_hash(&name);
        for pod in pods {
            let mounts_spare = pod
                .data
                .pointer("/spec/volumes")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .any(|volume| {
                    volume
                        .pointer("/persistentVolumeClaim/claimName")
                        .and_then(|value| value.as_str())
                        == Some(&storage)
                });
            let related = mounts_spare
                || pod.labels().get(TRACKING_LABEL) == Some(&hash)
                || refers_to(pod, &name, &uid);
            if related
                && (!sole_owner(pod, SANDBOX_VERSION, "Sandbox", &name, &uid)
                    || pod.labels().contains_key(CLAIM_LABEL))
            {
                return Err(rejected("native drain spare Pod ownership is ambiguous"));
            }
        }
        for volume in volumes {
            let related = volume.name_any() == storage || refers_to(volume, &name, &uid);
            if related
                && (volume.name_any() != storage
                    || !sole_owner(volume, SANDBOX_VERSION, "Sandbox", &name, &uid)
                    || volume.annotations().contains_key(NATIVE_HOME_ANNOTATION))
            {
                return Err(rejected("native drain spare PVC ownership is ambiguous"));
            }
        }
    }
    Ok(())
}

fn assignment_names(claim: &DynamicObject) -> impl Iterator<Item = &str> {
    [
        claim
            .data
            .pointer("/status/sandbox/name")
            .and_then(|v| v.as_str()),
        claim.annotations().get(ASSIGNMENT).map(String::as_str),
        claim.labels().get(ASSIGNMENT).map(String::as_str),
    ]
    .into_iter()
    .flatten()
}

fn sole_owner<K: Resource>(object: &K, version: &str, kind: &str, name: &str, uid: &str) -> bool {
    matches!(owners(object), [owner] if owner.api_version == version && owner.kind == kind
        && owner.name == name && owner.uid == uid && owner.controller == Some(true))
}

fn refers_to<K: Resource>(object: &K, name: &str, uid: &str) -> bool {
    owners(object)
        .iter()
        .any(|owner| owner.name == name || owner.uid == uid)
}
