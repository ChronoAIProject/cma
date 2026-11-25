use std::collections::{BTreeMap, BTreeSet};

use kube::{
    api::{DynamicObject, ListParams},
    Api, ResourceExt,
};

use crate::HomeVolumeAttachment;

use super::{
    api_error, rejected, unavailable, AgentSandboxPools, NativePoolClaim, NativePoolCleanup,
    NativePoolMember, Result, API_VERSION,
};

impl AgentSandboxPools {
    pub(in crate::agent_sandbox) async fn prove_member_pods_absent(&self, uid: &str) -> Result<()> {
        self.member_pod_absence_version(uid)
            .await?
            .map(|_| ())
            .ok_or_else(|| unavailable("native member Pods remain present"))
    }

    /// Read a complete namespace snapshot before checking the immutable owner UID.
    /// Tracking labels can change without updating the owning member.
    pub(in crate::agent_sandbox) async fn member_pod_absence_version(
        &self,
        uid: &str,
    ) -> Result<Option<String>> {
        let (pods, version) = complete_with_version(&self.pods).await?;
        let mut present = false;
        for pod in pods {
            if pod.namespace().as_deref() != Some(&self.config.namespace) {
                return Err(rejected("native Pod is outside its namespace"));
            }
            present |= pod
                .metadata
                .owner_references
                .iter()
                .flatten()
                .any(|owner| owner.uid == uid);
        }
        Ok((!present).then_some(version))
    }

    pub(in crate::agent_sandbox) async fn prove_retirement_volumes_absent(
        &self,
        member_uid: &str,
        home: &HomeVolumeAttachment,
    ) -> Result<()> {
        let mut present = false;
        for volume in complete(&self.volumes).await? {
            if volume.namespace().as_deref() != Some(&self.config.namespace) {
                return Err(rejected("retirement PVC is outside its namespace"));
            }
            let original_home =
                volume.metadata.uid.as_deref() == Some(home.receipt.storage_generation.as_str());
            if volume.metadata.name.as_deref() == Some(home.receipt.storage_reference.as_str())
                && !original_home
            {
                return Err(rejected("retirement home UID changed"));
            }
            // A native VCT still belongs to this retirement if its owner
            // reference was removed. Detached legacy homes release later.
            present |= (original_home && home.request.policy.is_native_vct())
                || volume
                    .metadata
                    .owner_references
                    .iter()
                    .flatten()
                    .any(|owner| owner.uid == member_uid);
        }
        if present {
            return Err(unavailable("native retirement PVCs remain present"));
        }
        Ok(())
    }

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
                let pvc_uid = if claim.request.home.policy.is_native_vct() {
                    let volume = self
                        .volumes
                        .get_opt(&format!("home-{}", member.name_any()))
                        .await
                        .map_err(api_error)?;
                    if let Some(volume) = volume {
                        let uid = volume
                            .uid()
                            .ok_or_else(|| rejected("native cleanup PVC UID missing"))?;
                        let mut assigned = claim.clone();
                        assigned.sandbox_name = Some(member.name_any());
                        let origin = super::NativePoolHomeOrigin {
                            claim: assigned,
                            sandbox_uid: member.uid().unwrap(),
                            pvc_uid: uid.clone(),
                        };
                        let mut observed = volume;
                        observed.metadata.deletion_timestamp = None;
                        self.validate_native_volume(&observed, &origin)?;
                        Some(uid)
                    } else {
                        None
                    }
                } else {
                    None
                };
                owned.push(NativePoolMember {
                    name: member.name_any(),
                    uid: member.uid().unwrap(),
                    pvc_uid,
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
        let mut live_members = None;
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
                // One fresh, complete member snapshot after the Pod snapshot
                // replaces a serial GET for every unrelated running Pod. Never
                // reuse discovery from a previous cleanup item or attempt.
                if live_members.is_none() {
                    let mut members = BTreeMap::new();
                    for member in complete(&self.members).await? {
                        if member.namespace().as_deref() != Some(&self.config.namespace)
                            || members.insert(member.name_any(), member).is_some()
                        {
                            return Err(rejected("native cleanup member inventory is ambiguous"));
                        }
                    }
                    live_members = Some(members);
                }
                let live = live_members
                    .as_ref()
                    .unwrap()
                    .get(&owner.name)
                    .filter(|member| member.uid().as_deref() == Some(&owner.uid));
                let Some(live) = live else {
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

pub(super) async fn complete<K>(api: &Api<K>) -> Result<Vec<K>>
where
    K: Clone + std::fmt::Debug + serde::de::DeserializeOwned + kube::Resource,
{
    complete_with_version(api).await.map(|(objects, _)| objects)
}

async fn complete_with_version<K>(api: &Api<K>) -> Result<(Vec<K>, String)>
where
    K: Clone + std::fmt::Debug + serde::de::DeserializeOwned + kube::Resource,
{
    let mut cursor = PageCursor::default();
    let mut objects = Vec::new();
    loop {
        let page = api.list(&cursor.params()).await.map_err(api_error)?;
        let complete = cursor.accept(&page)?;
        objects.extend(page.items);
        if complete {
            let version = cursor
                .version
                .ok_or_else(|| unavailable("native cleanup inventory has no version"))?;
            return Ok((objects, version));
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct PageCursor {
    cursor: Option<String>,
    cursors: BTreeSet<String>,
    identities: BTreeSet<String>,
    version: Option<String>,
    pages: usize,
}

impl PageCursor {
    pub(super) fn params(&self) -> ListParams {
        let params = ListParams::default().limit(200);
        self.cursor
            .as_ref()
            .map_or(params.clone(), |cursor| params.continue_token(cursor))
    }

    pub(super) fn accept<K>(&mut self, page: &kube::api::ObjectList<K>) -> Result<bool>
    where
        K: kube::Resource + Clone,
    {
        // Validate on a temporary checkpoint so an invalid later page cannot
        // partially advance identity, continuation or snapshot authority.
        let mut next = self.clone();
        let observed = page
            .metadata
            .resource_version
            .as_ref()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| unavailable("native cleanup inventory has no version"))?;
        if next.version.as_ref().is_some_and(|v| v != observed) {
            return Err(unavailable(
                "native cleanup inventory changed between pages",
            ));
        }
        next.version = Some(observed.clone());
        for object in &page.items {
            if !next.identities.insert(
                object
                    .uid()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| rejected("native cleanup object has no UID"))?,
            ) {
                return Err(rejected("native cleanup inventory repeats an identity"));
            }
            if next.identities.len() > 10_000 {
                return Err(unavailable("native cleanup inventory exceeds its bound"));
            }
        }
        next.pages += 1;
        next.cursor = page.metadata.continue_.clone().filter(|v| !v.is_empty());
        if let Some(cursor) = &next.cursor {
            if cursor.len() > 16_384 || !next.cursors.insert(cursor.clone()) {
                return Err(unavailable("native cleanup inventory did not advance"));
            }
            if next.pages >= 100 {
                return Err(unavailable(
                    "native cleanup inventory exceeds its page bound",
                ));
            }
        }
        let complete = next.cursor.is_none();
        *self = next;
        Ok(complete)
    }
}
