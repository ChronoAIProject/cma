use std::time::SystemTime;

use sha2::{Digest as _, Sha256};

use super::*;

/// Caller-selectable input only. Authority, root fence, operation identity and
/// timestamps are assigned by the service from authenticated/current state.
pub struct ShareCommand {
    pub selector: ResourceSelector,
    pub change: ShareChange,
    pub expected_share_revision: Option<u64>,
    pub idempotency_key: String,
}

impl ResourceSelector {
    pub fn target(&self) -> ResourceTarget {
        match self {
            Self::Agent { agent_id } => ResourceTarget::Agent {
                agent_id: agent_id.clone(),
            },
            Self::Sandbox { sandbox_id } => ResourceTarget::Sandbox {
                sandbox_id: sandbox_id.clone(),
            },
        }
    }
}

impl ResourceAccessService {
    /// Active direct grants only, sorted by server ID and bounded by the store's
    /// 128-grant capacity. Inherited grants are not silently presented as direct.
    pub async fn active_shares(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        selector: &ResourceSelector,
    ) -> Result<Vec<ResourceShare>, ResourceAccessError> {
        let snapshot = self
            .personal_owner(context, credential, &selector.target())
            .await?;
        Ok(snapshot
            .shares
            .into_iter()
            .filter(|share| &share.selector == selector)
            .collect())
    }

    /// Individual management reads retain revoked grants without exposing the
    /// owner's inventory to recipients or revealing another resource's shares.
    pub async fn read_share(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        selector: &ResourceSelector,
        id: &str,
    ) -> Result<ResourceShare, ResourceAccessError> {
        let snapshot = self
            .personal_owner(context, credential, &selector.target())
            .await?;
        let share = self
            .store
            .resource_share(id)
            .await?
            .filter(|share| {
                &share.selector == selector
                    && share.owner
                        == ResourceOwner::Person {
                            subject: context.caller.subject.clone(),
                        }
            })
            .ok_or(AccessError::NotFound)?;
        if !share.validate() || share.revision > snapshot.fence().policy_revision {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        Ok(share)
    }

    pub async fn change_share(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        command: ShareCommand,
    ) -> Result<ShareMutationReceipt, ResourceAccessError> {
        let started_at = SystemTime::now();
        // Redemption has its own caller-bound entrypoint; an owner cannot
        // manufacture a recipient through the management API.
        if matches!(command.change, ShareChange::RedeemLink { .. }) {
            return Err(ResourceAccessStoreError::Invalid.into());
        }
        if command.idempotency_key.is_empty()
            || command.idempotency_key.len() > 256
            || command
                .idempotency_key
                .bytes()
                .any(|byte| byte.is_ascii_control())
        {
            return Err(ResourceAccessStoreError::Invalid.into());
        }
        let snapshot = self
            .personal_owner(context, credential, &command.selector.target())
            .await?;
        let request = ShareMutation {
            selector: command.selector,
            owner: ResourceOwner::Person {
                subject: context.caller.subject.clone(),
            },
            actor: context.actor.clone(),
            key_sha256: format!("{:x}", Sha256::digest(command.idempotency_key.as_bytes())),
            change: command.change,
            expected_share_revision: command.expected_share_revision,
            policy_fence: snapshot.fence(),
            proposed_id: format!("shr_{}", uuid::Uuid::new_v4().simple()),
            at: crate::rfc3339_at(SystemTime::now()),
        };
        request.validate()?;
        // A retry has no new effect. Recover it after current ownership checks,
        // even if its organization disappeared or the original grant was revoked.
        if let Some(receipt) = self
            .store
            .share_mutation_receipt(&request.operation_id())
            .await?
        {
            return Ok(request.replay(receipt)?);
        }
        let organization = match &request.change {
            ShareChange::Create { target_org_id, .. } => Some(target_org_id),
            ShareChange::CreateLink { audience, .. } => audience.organization(),
            _ => None,
        };
        if let Some(target_org_id) = organization {
            // Only canonical current choices can be selected for a new grant.
            // Revocation and existing-grant management never depend on this IO.
            let evidence = self
                .organizations
                .organization_context(
                    &context.caller,
                    credential.ok_or(MembershipError::Unavailable)?,
                    target_org_id,
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
                || evidence.memberships[0].organization != *target_org_id
                || !evidence.memberships[0].is_current(&context.credential, SystemTime::now())
                || evidence.memberships[0].observed_at < started_at
            {
                return Err(MembershipError::InvalidEvidence.into());
            }
        }
        Ok(self.store.mutate_share(request).await?)
    }
}
