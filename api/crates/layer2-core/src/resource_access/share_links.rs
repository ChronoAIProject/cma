//! Revocable Agent-session links. Only a SHA-256 secret digest is durable;
//! possession is exchanged for a principal-bound grant, never owner identity.
use super::*;
use crate::{AgentId, SubjectId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, time::SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentShareAudience {
    Public,
    Organization { organization_id: SubjectId },
    Users { subjects: Vec<SubjectId> },
}
impl AgentShareAudience {
    pub fn validate(&self) -> bool {
        let valid = |id: &SubjectId| {
            !id.as_str().trim().is_empty()
                && id.as_str().len() <= 128
                && id.as_str().trim() == id.as_str()
                && !id.as_str().bytes().any(|b| b.is_ascii_control())
        };
        match self {
            Self::Public => true,
            Self::Organization { organization_id } => valid(organization_id),
            Self::Users { subjects } => {
                !subjects.is_empty()
                    && subjects.len() <= 100
                    && subjects.iter().all(valid)
                    && subjects.iter().collect::<BTreeSet<_>>().len() == subjects.len()
            }
        }
    }
    pub fn organization(&self) -> Option<&SubjectId> {
        match self {
            Self::Organization { organization_id } => Some(organization_id),
            _ => None,
        }
    }
    pub fn allows_subject(&self, subject: &SubjectId) -> bool {
        match self {
            Self::Users { subjects } => subjects.contains(subject),
            _ => true,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentShareLink {
    pub audience: AgentShareAudience,
    pub token_sha256: String,
    /// Individual records belong to this link; revoking it invalidates every
    /// accepted recipient without changing another link to the same Agent.
    pub accepted_subjects: Vec<SubjectId>,
}
impl AgentShareLink {
    pub fn validate(&self) -> bool {
        self.audience.validate()
            && digest_valid(&self.token_sha256)
            && self.accepted_subjects.len() <= 1000
            && self.accepted_subjects.iter().all(|s| {
                !s.as_str().is_empty() && s.as_str().len() <= 128 && self.audience.allows_subject(s)
            })
            && self.accepted_subjects.iter().collect::<BTreeSet<_>>().len()
                == self.accepted_subjects.len()
    }
}
pub(super) fn digest_valid(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn digest_equal(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .as_bytes()
            .iter()
            .zip(right.as_bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}
fn token_digest(token: &str) -> Result<String, ResourceAccessError> {
    // Browser/SDK supplies 32 CSPRNG bytes as lowercase hex, never a password.
    if !digest_valid(token) {
        return Err(ResourceAccessStoreError::Invalid.into());
    }
    Ok(format!("{:x}", Sha256::digest(token.as_bytes())))
}
impl ResourceShare {
    pub fn incoming_for(&self, subject: &SubjectId, organizations: &[SubjectId]) -> bool {
        if let Some(link) = &self.link {
            link.accepted_subjects.contains(subject)
                && link
                    .audience
                    .organization()
                    .is_none_or(|org| organizations.contains(org))
        } else {
            organizations.contains(&self.target_org_id)
        }
    }
    pub fn organization_recipient(&self) -> Option<&SubjectId> {
        match &self.link {
            Some(link) => link.audience.organization(),
            None => Some(&self.target_org_id),
        }
    }
}
pub struct CreateAgentShareLink {
    pub agent_id: AgentId,
    pub audience: AgentShareAudience,
    pub mode: ShareMode,
    pub token: String,
    pub idempotency_key: String,
}
impl ResourceAccessService {
    pub fn with_directory(mut self, directory: std::sync::Arc<crate::UserDirectory>) -> Self {
        self.directory = Some(directory);
        self
    }
    pub async fn share_recipients(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        agent_id: &AgentId,
        query: &str,
    ) -> Result<Vec<crate::PublicIdentity>, ResourceAccessError> {
        self.personal_owner(
            context,
            credential,
            &ResourceTarget::Agent {
                agent_id: agent_id.clone(),
            },
        )
        .await?;
        let directory = self
            .directory
            .as_ref()
            .ok_or(ResourceAccessStoreError::Unavailable)?;
        directory
            .share_recipients(query)
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable.into())
    }
    pub async fn share_owner_identity(
        &self,
        share: &ResourceShare,
    ) -> Result<crate::PublicIdentity, ResourceAccessError> {
        let ResourceOwner::Person { subject } = &share.owner else {
            return Err(ResourceAccessStoreError::Corrupt.into());
        };
        let found = if let Some(directory) = &self.directory {
            directory
                .user(subject)
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?
        } else {
            None
        };
        Ok(found
            .map(|r| r.identity)
            .unwrap_or_else(|| crate::PublicIdentity {
                subject: subject.clone(),
                display_name: Some("CMA user".into()),
                avatar_url: None,
            }))
    }
    pub async fn create_agent_share_link(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        command: CreateAgentShareLink,
    ) -> Result<ShareMutationReceipt, ResourceAccessError> {
        let CreateAgentShareLink {
            agent_id,
            audience,
            mode,
            token,
            idempotency_key,
        } = command;
        if !audience.validate() || !matches!(mode, ShareMode::View | ShareMode::ViewWrite) {
            return Err(ResourceAccessStoreError::Invalid.into());
        }
        self.personal_owner(
            context,
            credential,
            &ResourceTarget::Agent {
                agent_id: agent_id.clone(),
            },
        )
        .await?;
        // The owner can select only existing, canonical CMA identities.
        if let AgentShareAudience::Users { subjects } = &audience {
            let directory = self
                .directory
                .as_ref()
                .ok_or(ResourceAccessStoreError::Unavailable)?;
            for subject in subjects {
                let user = directory
                    .user(subject)
                    .await
                    .map_err(|_| ResourceAccessStoreError::Unavailable)?;
                if user.is_none_or(|u| u.identity.subject != *subject) {
                    return Err(ResourceAccessStoreError::Invalid.into());
                }
            }
        }
        self.change_share(
            context,
            credential,
            ShareCommand {
                selector: ResourceSelector::Agent { agent_id },
                change: ShareChange::CreateLink {
                    audience,
                    mode,
                    token_sha256: token_digest(&token)?,
                },
                expected_share_revision: None,
                idempotency_key,
            },
        )
        .await
    }
    pub async fn redeem_agent_share_link(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        share_id: &str,
        token: &str,
    ) -> Result<ResourceShare, ResourceAccessError> {
        let started_at = SystemTime::now();
        self.require_recipient_available(&context.caller.subject)
            .await?;
        if !context.validate_principal()
            || context.actor.ordinary_subject() != Some(&context.caller.subject)
            || !context
                .caller
                .authorization
                .allows(crate::Capability::UseOwnSandboxes)
        {
            return Err(AccessError::NotFound.into());
        }
        let digest = token_digest(token)?;
        for _ in 0..3 {
            let share = self
                .store
                .resource_share(share_id)
                .await?
                .filter(|s| s.revoked_at.is_none() && s.validate())
                .ok_or(AccessError::NotFound)?;
            let link = share
                .link
                .as_ref()
                .filter(|l| {
                    digest_equal(&l.token_sha256, &digest)
                        && l.audience.allows_subject(&context.caller.subject)
                })
                .ok_or(AccessError::NotFound)?;
            if let Some(org) = link.audience.organization() {
                let evidence = self
                    .organizations
                    .organization_context(
                        &context.caller,
                        credential.ok_or(MembershipError::Unavailable)?,
                        org,
                    )
                    .await?;
                if !evidence.validate_principal()
                    || evidence.credential != context.credential
                    || evidence.caller.subject != context.caller.subject
                    || !evidence
                        .caller
                        .authorization
                        .allows(crate::Capability::UseOwnSandboxes)
                    || evidence.memberships.len() != 1
                    || evidence.memberships[0].organization != *org
                    || !evidence.memberships[0].is_current(&context.credential, SystemTime::now())
                    || evidence.memberships[0].observed_at < started_at
                {
                    return Err(MembershipError::InvalidEvidence.into());
                }
            }
            let snapshot = self
                .store
                .resource_access_snapshot(&share.selector.target())
                .await?;
            if !snapshot
                .shares
                .iter()
                .any(|current| current.id == share.id && current.revision == share.revision)
            {
                continue;
            }
            if link.accepted_subjects.contains(&context.caller.subject) {
                return Ok(share);
            }
            let request = ShareMutation {
                selector: share.selector.clone(),
                owner: share.owner.clone(),
                actor: context.actor.clone(),
                key_sha256: format!(
                    "{:x}",
                    Sha256::digest(format!(
                        "redeem:{}:{}:{}",
                        share.id, context.caller.subject, share.revision
                    ))
                ),
                change: ShareChange::RedeemLink {
                    share_id: share.id.clone(),
                    token_sha256: digest.clone(),
                    subject: context.caller.subject.clone(),
                },
                expected_share_revision: Some(share.revision),
                policy_fence: snapshot.fence(),
                proposed_id: share.id,
                at: crate::now_rfc3339(),
            };
            match self.store.mutate_share(request).await {
                Ok(receipt) => return Ok(receipt.share),
                Err(ResourceAccessStoreError::Conflict) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ResourceAccessStoreError::Conflict.into())
    }
}
