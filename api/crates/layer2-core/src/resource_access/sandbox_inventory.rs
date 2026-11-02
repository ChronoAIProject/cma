use super::*;
use crate::sandbox::{store::PageRequest, OwnerSandboxQuery, Sandbox};
use crate::{Capability, SubjectId};
use futures_util::{stream, StreamExt, TryStreamExt};
use std::time::{Duration, SystemTime};

pub(crate) struct SandboxInventoryCandidates {
    pub(crate) membership: InventoryMemberships,
    pub(crate) items: Vec<AuthorizedResource>,
}

impl ResourceAccessSnapshot {
    /// Explicit incoming parent grants only; narrow Agent grants cannot reveal a Sandbox.
    pub fn shared_sandbox(
        &self,
        recipient: &SubjectId,
        organizations: &[SubjectId],
    ) -> Result<&Sandbox, ResourceAccessStoreError> {
        let sandbox = &self.lineage.sandbox;
        let scope = self.scope(&ResourceTarget::Sandbox {
            sandbox_id: sandbox.id.clone(),
        })?;
        if matches!(&scope.owner, ResourceOwner::Person { subject } if subject == recipient)
            || !self.shares.iter().any(|share| {
                organizations.contains(&share.target_org_id)
                    && share.revoked_at.is_none()
                    && share.selector.covers(&scope)
            })
        {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(sandbox)
    }

    /// Whole-parent visibility is required even if a child Agent is shared.
    pub fn organization_sandbox(
        &self,
        organization: &SubjectId,
    ) -> Result<&Sandbox, ResourceAccessStoreError> {
        let sandbox = &self.lineage.sandbox;
        let scope = self.scope(&ResourceTarget::Sandbox {
            sandbox_id: sandbox.id.clone(),
        })?;
        let owned = matches!(&scope.owner, ResourceOwner::Organization { subject } if subject == organization);
        if !owned
            && !self.shares.iter().any(|share| {
                &share.target_org_id == organization
                    && share.revoked_at.is_none()
                    && share.selector.covers(&scope)
            })
        {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(sandbox)
    }
}

impl ResourceAccessService {
    /// Internal unpaginated evidence. The hierarchy reader must retain the
    /// lookahead and call finish_sandbox_inventory after all count/projection IO.
    pub(crate) async fn sandbox_inventory_candidates(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        organization: Option<&SubjectId>,
        query: &OwnerSandboxQuery,
    ) -> Result<SandboxInventoryCandidates, ResourceAccessError> {
        if !context.validate_principal() {
            return Err(AccessError::NotFound.into());
        }
        if !context
            .caller
            .authorization
            .allows(Capability::UseOwnSandboxes)
        {
            return Err(AccessError::Forbidden.into());
        }
        PageRequest::new(query.page.limit, query.page.cursor.clone())
            .map_err(|_| ResourceAccessStoreError::Invalid)?;
        if query
            .page
            .cursor
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 256 || id.trim() != id)
        {
            return Err(ResourceAccessStoreError::Invalid.into());
        }
        let membership =
            InventoryMemberships::load(self, context, credential, organization).await?;
        let snapshots = match organization {
            Some(organization) => self.store.organization_sandboxes(organization, query).await,
            None => {
                self.store
                    .shared_sandboxes(&context.caller.subject, &membership.ids(), query)
                    .await
            }
        };
        membership.require(self)?;
        let snapshots = snapshots?;
        if snapshots.len() > usize::from(query.page.limit) + 1 {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        let mut previous = query.page.cursor.clone();
        let mut items = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            let sandbox = match organization {
                Some(organization) => snapshot.organization_sandbox(organization)?,
                None => snapshot.shared_sandbox(&context.caller.subject, &membership.ids())?,
            };
            if !query.matches_status(sandbox.status)
                || previous
                    .as_deref()
                    .is_some_and(|id| id >= sandbox.id.as_str())
            {
                return Err(ResourceAccessStoreError::Corrupt.into());
            }
            previous = Some(sandbox.id.to_string());
            let scope = snapshot.scope(&ResourceTarget::Sandbox {
                sandbox_id: sandbox.id.clone(),
            })?;
            let decision = AccessDecision::evaluate(
                &membership.context,
                &scope,
                &snapshot.shares,
                AccessPurpose::Read,
                SystemTime::now(),
            )?;
            decision.require(ResourceCapability::ViewDetails)?;
            items.push(AuthorizedResource {
                snapshot,
                decision,
                binding: None,
            });
        }
        membership.require(self)?;
        Ok(SandboxInventoryCandidates { membership, items })
    }

    pub(crate) async fn finish_sandbox_inventory(
        &self,
        candidates: &SandboxInventoryCandidates,
        query: &OwnerSandboxQuery,
    ) -> Result<(), ResourceAccessError> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut checks = Vec::new();
            for value in &candidates.items {
                checks.push(self.validate_sandbox_candidate(value, query));
            }
            stream::iter(checks)
                .buffer_unordered(8)
                .try_collect::<Vec<_>>()
                .await?;
            candidates.membership.require(self)?;
            for item in &candidates.items {
                item.decision.require(ResourceCapability::ViewDetails)?;
            }
            Ok(())
        })
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)?
    }
    async fn validate_sandbox_candidate(
        &self,
        value: &AuthorizedResource,
        query: &OwnerSandboxQuery,
    ) -> Result<(), ResourceAccessError> {
        let current = self
            .current_read_snapshot(
                value,
                &ResourceTarget::Sandbox {
                    sandbox_id: value.snapshot.lineage.sandbox.id.clone(),
                },
                ResourceCapability::ViewDetails,
            )
            .await?;
        if !query.matches_status(current.lineage.sandbox.status) {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        Ok(())
    }
}
