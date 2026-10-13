use std::collections::BTreeSet;

use kube::{
    api::{DynamicObject, ListParams},
    Api, ResourceExt,
};

use super::{
    api_error, rejected, unavailable, AgentSandboxPools, NativePoolClaim, NativePoolCleanup,
    NativePoolMember, Result, API_VERSION,
};

impl AgentSandboxPools {
    pub(super) fn cleanup_member(
        &self,
        member: &DynamicObject,
        claim: &NativePoolClaim,
    ) -> Result<()> {
        let owners = member
            .metadata
            .owner_references
            .as_deref()
            .unwrap_or_default();
        if member.namespace().as_deref() != Some(&self.config.namespace)
            || member.types.as_ref().is_none_or(|kind| {
                kind.api_version != "agents.x-k8s.io/v1beta1" || kind.kind != "Sandbox"
            })
            || member
                .uid()
                .as_deref()
                .is_none_or(|uid| !super::super::model::dns_label(uid))
            || member
                .resource_version()
                .as_deref()
                .is_none_or(str::is_empty)
            || !super::super::model::dns_label(&member.name_any())
            || owners.len() != 1
            || !owners.iter().all(|owner| {
                owner.api_version == API_VERSION
                    && owner.kind == "SandboxClaim"
                    && owner.uid == claim.uid
                    && owner.name == claim.name
                    && owner.controller == Some(true)
            })
        {
            return Err(rejected("native cleanup member has another owner"));
        }
        Ok(())
    }

    pub(super) async fn cleanup_members(
        &self,
        claim: &NativePoolClaim,
    ) -> Result<Vec<NativePoolMember>> {
        let mut owned = Vec::new();
        for member in complete(&self.members).await? {
            let owners = member
                .metadata
                .owner_references
                .as_deref()
                .unwrap_or_default();
            let assigned = claim.sandbox_name.as_deref() == Some(&member.name_any());
            if owners.iter().any(|owner| owner.uid == claim.uid) || assigned {
                self.cleanup_member(&member, claim)?;
                owned.push(NativePoolMember {
                    name: member.name_any(),
                    uid: member.uid().unwrap(),
                });
            }
        }
        Ok(owned)
    }

    pub(super) async fn cleanup_pods_absent(&self, original: &NativePoolCleanup) -> Result<()> {
        // Include Pods whose parent CRD has already disappeared. Unadmitted
        // members may be garbage-collected before status records their name.
        // An orphan Sandbox-owned Pod is therefore conservatively pending,
        // even when its old parent was not in our first discovery snapshot.
        for pod in complete(&self.pods).await? {
            if pod.namespace().as_deref() != Some(&self.config.namespace) {
                return Err(rejected("native cleanup Pod is outside its namespace"));
            }
            for owner in pod.metadata.owner_references.as_deref().unwrap_or_default() {
                if owner.kind != "Sandbox" || !owner.api_version.starts_with("agents.x-k8s.io/") {
                    continue;
                }
                if original.members.iter().any(|m| m.uid == owner.uid)
                    || original
                        .claim
                        .as_ref()
                        .and_then(|c| c.sandbox_name.as_ref())
                        == Some(&owner.name)
                {
                    return Err(unavailable("native member Pods remain present"));
                }
                let live = self.members.get_opt(&owner.name).await.map_err(api_error)?;
                let Some(live) = live.filter(|v| v.uid().as_deref() == Some(&owner.uid)) else {
                    return Err(unavailable("native Pod parent finality remains uncertain"));
                };
                if original.claim.as_ref().is_some_and(|claim| {
                    live.metadata
                        .owner_references
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .any(|o| o.uid == claim.uid)
                }) {
                    return Err(unavailable("native claim still owns a running member"));
                }
            }
        }
        Ok(())
    }
}

pub(super) async fn complete(api: &Api<DynamicObject>) -> Result<Vec<DynamicObject>> {
    let mut params = ListParams::default().limit(200);
    let mut cursors = BTreeSet::new();
    let mut identities = BTreeSet::new();
    let mut version = None;
    let mut objects = Vec::new();
    for _ in 0..100 {
        let page = api.list(&params).await.map_err(api_error)?;
        let observed = page
            .metadata
            .resource_version
            .filter(|v| !v.is_empty())
            .ok_or_else(|| unavailable("native cleanup inventory has no version"))?;
        if version.as_ref().is_some_and(|v| v != &observed) {
            return Err(unavailable(
                "native cleanup inventory changed between pages",
            ));
        }
        version = Some(observed);
        for object in page.items {
            if !identities.insert(
                object
                    .uid()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| rejected("native cleanup object has no UID"))?,
            ) {
                return Err(rejected("native cleanup inventory repeats an identity"));
            }
            objects.push(object);
            if objects.len() > 10_000 {
                return Err(unavailable("native cleanup inventory exceeds its bound"));
            }
        }
        let Some(cursor) = page.metadata.continue_.filter(|v| !v.is_empty()) else {
            return Ok(objects);
        };
        if cursor.len() > 16_384 || !cursors.insert(cursor.clone()) {
            return Err(unavailable("native cleanup inventory did not advance"));
        }
        params = params.continue_token(&cursor);
    }
    Err(unavailable(
        "native cleanup inventory exceeds its page bound",
    ))
}
