use super::*;
use crate::sandbox::{store::PageRequest, OwnerSandboxQuery, Sandbox};
use crate::{Capability, SubjectId};
use futures_util::{stream, StreamExt, TryStreamExt};
use std::time::{Duration, SystemTime};

pub(crate) struct SandboxInventoryCandidates {
    pub(crate) current: CallerContext,
    pub(crate) items: Vec<AuthorizedResource>,
}

impl ResourceAccessSnapshot {
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
    pub(crate) async fn organization_sandbox_candidates(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        organization: &SubjectId,
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
        if organization.as_str().trim().is_empty()
            || organization.as_str().len() > 128
            || organization.as_str().trim() != organization.as_str()
        {
            return Err(ResourceAccessStoreError::Invalid.into());
        }
        let current = self
            .inventory_context(context, credential, organization)
            .await?;
        let snapshots = tokio::time::timeout(
            Duration::from_secs(15),
            self.store.organization_sandboxes(organization, query),
        )
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)??;
        if snapshots.len() > usize::from(query.page.limit) + 1 {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        let mut previous = query.page.cursor.clone();
        let mut items = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            let sandbox = snapshot.organization_sandbox(organization)?;
            if query.status.is_some_and(|status| sandbox.status != status)
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
                &current,
                &scope,
                &snapshot.shares,
                AccessPurpose::Read,
                SystemTime::now(),
            )?;
            decision.require(ResourceCapability::ViewDetails)?;
            items.push(AuthorizedResource { snapshot, decision });
        }
        self.require_inventory_membership(&current, organization)?;
        Ok(SandboxInventoryCandidates { current, items })
    }

    pub(crate) async fn finish_sandbox_inventory(
        &self,
        candidates: &SandboxInventoryCandidates,
        organization: &SubjectId,
    ) -> Result<(), ResourceAccessError> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut checks = Vec::new();
            for value in &candidates.items {
                checks.push(self.validate_sandbox_candidate(value, organization));
            }
            stream::iter(checks)
                .buffer_unordered(8)
                .try_collect::<Vec<_>>()
                .await?;
            self.require_inventory_membership(&candidates.current, organization)
        })
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)?
    }
    async fn validate_sandbox_candidate(
        &self,
        value: &AuthorizedResource,
        organization: &SubjectId,
    ) -> Result<(), ResourceAccessError> {
        let sandbox = value.snapshot.organization_sandbox(organization)?;
        self.validate_read_result(
            value,
            &ResourceTarget::Sandbox {
                sandbox_id: sandbox.id.clone(),
            },
            ResourceCapability::ViewDetails,
        )
        .await
    }
}
