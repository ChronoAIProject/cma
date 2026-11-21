use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{SandboxId, SubjectId};

use super::{
    ResourceAccessSnapshot, ResourceAccessStoreError, ResourceActor, ResourceOwner,
    ResourcePolicyFence, ResourcePolicyRecord, ResourceSelector, ResourceShare, ResourceTarget,
    ShareMode,
};

pub const MAX_ACTIVE_RESOURCE_SHARES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShareChange {
    Create {
        target_org_id: SubjectId,
        mode: ShareMode,
    },
    CreateLink {
        audience: super::AgentShareAudience,
        mode: ShareMode,
        token_sha256: String,
    },
    RedeemLink {
        share_id: String,
        token_sha256: String,
        subject: SubjectId,
    },
    SetMode {
        share_id: String,
        mode: ShareMode,
    },
    Revoke {
        share_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct ShareMutation {
    pub selector: ResourceSelector,
    pub owner: ResourceOwner,
    pub actor: ResourceActor,
    pub key_sha256: String,
    pub change: ShareChange,
    pub expected_share_revision: Option<u64>,
    pub policy_fence: ResourcePolicyFence,
    /// New IDs/timestamps are server-generated and excluded from the request's
    /// semantic fingerprint so an ambiguous create can recover its original ID.
    pub proposed_id: String,
    pub at: String,
}
impl ShareMutation {
    pub fn target(&self) -> ResourceTarget {
        match &self.selector {
            ResourceSelector::Agent { agent_id } => ResourceTarget::Agent {
                agent_id: agent_id.clone(),
            },
            ResourceSelector::Sandbox { sandbox_id } => ResourceTarget::Sandbox {
                sandbox_id: sandbox_id.clone(),
            },
        }
    }
    pub fn operation_id(&self) -> String {
        hash(&(
            "resource-share-command-v1",
            &self.owner,
            &self.selector,
            &self.key_sha256,
        ))
    }
    pub fn fingerprint(&self) -> String {
        hash(&(
            "resource-share-input-v1",
            &self.owner,
            &self.selector,
            &self.actor,
            &self.change,
            self.expected_share_revision,
        ))
    }
    pub fn validate(&self) -> Result<(), ResourceAccessStoreError> {
        let ResourceOwner::Person { subject } = &self.owner else {
            return Err(ResourceAccessStoreError::Invalid);
        };
        if !matches!(&self.actor,ResourceActor::Human {subject:actor}|ResourceActor::Api {subject:actor,delegation_id:None} if actor==subject || matches!(&self.change, ShareChange::RedeemLink { subject: recipient, .. } if actor == recipient))
            || !hex_digest(&self.key_sha256)
            || self.proposed_id.trim().is_empty()
            || self.proposed_id.len() > 128
            || crate::seconds_from_rfc3339(&self.at).is_none()
            || self.policy_fence.sandbox_id.as_str().trim().is_empty()
            || crate::seconds_from_rfc3339(&self.policy_fence.sandbox_created_at).is_none()
            || self.policy_fence.policy_revision > i64::MAX as u64
        {
            return Err(ResourceAccessStoreError::Invalid);
        }
        match &self.change {
            ShareChange::Create { target_org_id, .. }
                if self.expected_share_revision.is_none()
                    && !target_org_id.as_str().trim().is_empty() => {}
            ShareChange::CreateLink {
                audience,
                mode,
                token_sha256,
            } if self.expected_share_revision.is_none()
                && audience.validate()
                && matches!(mode, ShareMode::View | ShareMode::ViewWrite)
                && hex_digest(token_sha256)
                && matches!(self.selector, ResourceSelector::Agent { .. }) => {}
            ShareChange::RedeemLink {
                share_id,
                token_sha256,
                subject,
            } if !share_id.is_empty()
                && !subject.as_str().is_empty()
                && hex_digest(token_sha256)
                && self.actor.ordinary_subject() == Some(subject)
                && self
                    .expected_share_revision
                    .is_some_and(|r| r > 0 && r < i64::MAX as u64) => {}
            ShareChange::SetMode { share_id, .. } | ShareChange::Revoke { share_id }
                if !share_id.trim().is_empty()
                    && self
                        .expected_share_revision
                        .is_some_and(|revision| revision > 0 && revision < i64::MAX as u64) => {}
            _ => return Err(ResourceAccessStoreError::Invalid),
        }
        Ok(())
    }
    pub fn replay(
        &self,
        receipt: ShareMutationReceipt,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        receipt.validate()?;
        if receipt.share.owner != self.owner
            || receipt.share.selector != self.selector
            || receipt.actor != self.actor
            || receipt.sandbox_id != self.policy_fence.sandbox_id
            || receipt.sandbox_created_at != self.policy_fence.sandbox_created_at
            || receipt.operation_id != self.operation_id()
            || receipt.command_fingerprint != self.fingerprint()
        {
            return Err(ResourceAccessStoreError::IdempotencyConflict);
        }
        let effect_matches = match &self.change {
            ShareChange::Create {
                target_org_id,
                mode,
            } => {
                &receipt.share.target_org_id == target_org_id
                    && receipt.share.mode == *mode
                    && receipt.share.revision == 1
                    && receipt.share.created_at == receipt.at
                    && receipt.share.revoked_at.is_none()
            }
            ShareChange::CreateLink {
                audience,
                mode,
                token_sha256,
            } => {
                receipt.share.mode == *mode
                    && receipt.share.revision == 1
                    && receipt.share.revoked_at.is_none()
                    && receipt.share.link.as_ref().is_some_and(|l| {
                        &l.audience == audience
                            && &l.token_sha256 == token_sha256
                            && l.accepted_subjects.is_empty()
                    })
            }
            ShareChange::RedeemLink {
                share_id,
                token_sha256,
                subject,
            } => {
                &receipt.share.id == share_id
                    && receipt.share.revoked_at.is_none()
                    && receipt.share.link.as_ref().is_some_and(|l| {
                        &l.token_sha256 == token_sha256 && l.accepted_subjects.contains(subject)
                    })
                    && self.expected_share_revision.and_then(|v| v.checked_add(1))
                        == Some(receipt.share.revision)
            }
            ShareChange::SetMode { share_id, mode } => {
                &receipt.share.id == share_id
                    && receipt.share.mode == *mode
                    && receipt.share.revoked_at.is_none()
                    && self
                        .expected_share_revision
                        .and_then(|old| old.checked_add(1))
                        == Some(receipt.share.revision)
            }
            ShareChange::Revoke { share_id } => {
                &receipt.share.id == share_id
                    && receipt.share.revoked_at.is_some()
                    && self
                        .expected_share_revision
                        .and_then(|old| old.checked_add(1))
                        == Some(receipt.share.revision)
            }
        };
        if !effect_matches {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(receipt)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareMutationReceipt {
    pub operation_id: String,
    pub command_fingerprint: String,
    pub sandbox_id: SandboxId,
    pub sandbox_created_at: String,
    pub share: ResourceShare,
    pub actor: ResourceActor,
    pub at: String,
    pub policy_revision: u64,
}
impl ShareMutationReceipt {
    pub fn validate(&self) -> Result<(), ResourceAccessStoreError> {
        if !hex_digest(&self.operation_id)
            || !hex_digest(&self.command_fingerprint)
            || !self.share.validate()
            || !matches!(&self.share.owner,
                ResourceOwner::Person { subject } if self.actor.ordinary_subject() == Some(subject) || self.share.link.as_ref().is_some_and(|l| self.actor.ordinary_subject().is_some_and(|actor| l.accepted_subjects.contains(actor))))
            || self.sandbox_id.as_str().trim().is_empty()
            || crate::seconds_from_rfc3339(&self.sandbox_created_at).is_none()
            || crate::seconds_from_rfc3339(&self.at).is_none()
            || self.policy_revision == 0
            || self.policy_revision > i64::MAX as u64
            || self.share.revision > self.policy_revision
            || crate::seconds_from_rfc3339(&self.at)
                < crate::seconds_from_rfc3339(&self.share.created_at)
            || self.share.revoked_at.as_ref().is_some_and(|revoked| {
                crate::seconds_from_rfc3339(revoked) > crate::seconds_from_rfc3339(&self.at)
            })
            || matches!(&self.share.selector, ResourceSelector::Sandbox { sandbox_id }
                if sandbox_id != &self.sandbox_id)
        {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(())
    }
}

pub struct ShareMutationPlan {
    pub policy: ResourcePolicyRecord,
    pub receipt: ShareMutationReceipt,
}

/// Applied under the store's resource/policy transaction. Both stores use this
/// transition so CAS, isolation, revocation and immutable attribution agree.
pub fn plan_share_mutation(
    request: &ShareMutation,
    snapshot: &ResourceAccessSnapshot,
    previous: Option<&ResourceShare>,
    active_share_count: usize,
) -> Result<ShareMutationPlan, ResourceAccessStoreError> {
    request.validate()?;
    let scope = snapshot.scope(&request.target())?;
    if scope.owner != request.owner {
        return Err(ResourceAccessStoreError::NotFound);
    }
    if snapshot.fence() != request.policy_fence {
        return Err(ResourceAccessStoreError::Conflict);
    }
    let policy_revision = request
        .policy_fence
        .policy_revision
        .checked_add(1)
        .filter(|revision| *revision <= i64::MAX as u64)
        .ok_or(ResourceAccessStoreError::Conflict)?;
    let mut share = match &request.change {
        ShareChange::Create {
            target_org_id,
            mode,
        } => {
            if previous.is_some() {
                return Err(ResourceAccessStoreError::Conflict);
            }
            if active_share_count >= MAX_ACTIVE_RESOURCE_SHARES {
                return Err(ResourceAccessStoreError::Capacity);
            }
            ResourceShare {
                id: request.proposed_id.clone(),
                owner: request.owner.clone(),
                selector: request.selector.clone(),
                target_org_id: target_org_id.clone(),
                link: None,
                mode: *mode,
                issuer_actor: request.actor.clone(),
                created_at: request.at.clone(),
                revoked_at: None,
                revision: 1,
            }
        }
        ShareChange::CreateLink {
            audience,
            mode,
            token_sha256,
        } => {
            if previous.is_some() || active_share_count >= MAX_ACTIVE_RESOURCE_SHARES {
                return Err(ResourceAccessStoreError::Capacity);
            }
            ResourceShare {
                id: request.proposed_id.clone(),
                owner: request.owner.clone(),
                selector: request.selector.clone(),
                target_org_id: SubjectId::new(""),
                mode: *mode,
                link: Some(super::AgentShareLink {
                    audience: audience.clone(),
                    token_sha256: token_sha256.clone(),
                    accepted_subjects: Vec::new(),
                }),
                issuer_actor: request.actor.clone(),
                created_at: request.at.clone(),
                revoked_at: None,
                revision: 1,
            }
        }
        ShareChange::SetMode { share_id, .. }
        | ShareChange::Revoke { share_id }
        | ShareChange::RedeemLink { share_id, .. } => {
            let share = previous
                .filter(|share| {
                    &share.id == share_id
                        && share.owner == request.owner
                        && share.selector == request.selector
                })
                .ok_or(ResourceAccessStoreError::NotFound)?;
            if !share.validate() {
                return Err(ResourceAccessStoreError::Corrupt);
            }
            if Some(share.revision) != request.expected_share_revision {
                return Err(ResourceAccessStoreError::Conflict);
            }
            let mut next = share.clone();
            next.revision = next
                .revision
                .checked_add(1)
                .filter(|value| *value <= i64::MAX as u64)
                .ok_or(ResourceAccessStoreError::Conflict)?;
            next
        }
    };
    match &request.change {
        ShareChange::SetMode { mode, .. } => {
            if share.revoked_at.is_some() {
                return Err(ResourceAccessStoreError::Conflict);
            }
            share.mode = *mode;
        }
        ShareChange::Revoke { .. } => {
            share.revoked_at.get_or_insert(request.at.clone());
        }
        ShareChange::RedeemLink {
            token_sha256,
            subject,
            ..
        } => {
            if share.revoked_at.is_some() {
                return Err(ResourceAccessStoreError::NotFound);
            }
            let link = share
                .link
                .as_mut()
                .filter(|link| {
                    super::share_links::digest_equal(&link.token_sha256, token_sha256)
                        && link.audience.allows_subject(subject)
                })
                .ok_or(ResourceAccessStoreError::NotFound)?;
            if link.accepted_subjects.len() >= 1000 {
                return Err(ResourceAccessStoreError::Capacity);
            }
            if !link.accepted_subjects.contains(subject) {
                link.accepted_subjects.push(subject.clone());
            }
        }
        ShareChange::Create { .. } | ShareChange::CreateLink { .. } => {}
    }
    if !share.validate() {
        return Err(ResourceAccessStoreError::Invalid);
    }
    if share.revoked_at.is_none() {
        share
            .require_safe_interaction(&scope, &snapshot.shares)
            .map_err(|_| ResourceAccessStoreError::UnsafeInteractiveShare)?;
    }
    let policy = ResourcePolicyRecord {
        sandbox_id: scope.sandbox_id.clone(),
        sandbox_created_at: snapshot.lineage.sandbox.created_at.clone(),
        owner: scope.owner,
        organization_evidence: None,
        revision: policy_revision,
    };
    policy.validate()?;
    let receipt = ShareMutationReceipt {
        operation_id: request.operation_id(),
        command_fingerprint: request.fingerprint(),
        sandbox_id: scope.sandbox_id,
        sandbox_created_at: snapshot.lineage.sandbox.created_at.clone(),
        share,
        actor: request.actor.clone(),
        at: request.at.clone(),
        policy_revision,
    };
    receipt.validate()?;
    Ok(ShareMutationPlan { policy, receipt })
}

fn hash(value: &impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("typed share identity serializes"))
    )
}
fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[async_trait::async_trait]
pub trait ResourceShareStore: super::ResourceAccessStore {
    /// An immutable result is only replayable after current resource ownership
    /// has been authorized. It never grants authority for another effect.
    async fn share_mutation_receipt(
        &self,
        operation_id: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError>;

    async fn mutate_share(
        &self,
        request: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError>;
    /// Read management records (including revoked grants); authorize the owning
    /// resource before exposing them. Recipients do not receive this inventory.
    async fn resource_share(
        &self,
        id: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError>;
}
