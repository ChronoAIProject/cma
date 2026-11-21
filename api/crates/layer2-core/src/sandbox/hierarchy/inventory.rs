use super::*;
use crate::resource_access::{AuthorizedResource, CallerContext, ResourceAccessStoreError};
use std::time::Duration;

impl HierarchyReadService {
    /// Current organization scope, with authorized counts and no narrow-grant
    /// parent discovery. The hidden candidate is fenced after all hierarchy IO.
    pub async fn organization_list_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        organization: &SubjectId,
        query: &OwnerSandboxQuery,
    ) -> Result<Page<SandboxInventoryReading>, HierarchyReadError> {
        self.inventory_authorized(context, credential, Some(organization), query)
            .await
    }

    /// Explicit incoming whole-Sandbox shares across current organizations.
    pub async fn shared_list_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        query: &OwnerSandboxQuery,
    ) -> Result<Page<SandboxInventoryReading>, HierarchyReadError> {
        self.inventory_authorized(context, credential, None, query)
            .await
    }

    async fn inventory_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        organization: Option<&SubjectId>,
        query: &OwnerSandboxQuery,
    ) -> Result<Page<SandboxInventoryReading>, HierarchyReadError> {
        tokio::time::timeout(
            Duration::from_secs(15),
            self.inventory_inner(context, credential, organization, query),
        )
        .await
        .map_err(|_| HierarchyReadError::Unavailable("Sandbox inventory timed out".into()))?
    }

    async fn inventory_inner(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        organization: Option<&SubjectId>,
        query: &OwnerSandboxQuery,
    ) -> Result<Page<SandboxInventoryReading>, HierarchyReadError> {
        let access = self.access()?;
        let candidates = access
            .sandbox_inventory_candidates(context, credential, organization, query)
            .await
            .map_err(authorized::access_error)?;
        let projected = tokio::time::timeout(Duration::from_secs(15), async {
            let mut reads = Vec::new();
            for held in candidates.items.iter().take(usize::from(query.page.limit)) {
                reads.push(self.organization_inventory_row(held, context, query));
            }
            stream::iter(reads)
                .buffered(8)
                .try_collect::<Vec<_>>()
                .await
        })
        .await
        .map_err(|_| {
            HierarchyReadError::Unavailable("organization hierarchy inventory timed out".into())
        })
        .and_then(|result| result);
        // Fence success, private dependency failures and timeout alike. Do not
        // discard the hidden lookahead until after this check.
        access
            .finish_sandbox_inventory(&candidates, query)
            .await
            .map_err(authorized::access_error)?;
        let items = projected?;
        let next = (candidates.items.len() > usize::from(query.page.limit)).then(|| {
            candidates.items[usize::from(query.page.limit) - 1]
                .snapshot
                .lineage
                .sandbox
                .id
                .to_string()
        });
        Ok(Page { items, next })
    }

    async fn organization_inventory_row(
        &self,
        held: &AuthorizedResource,
        context: &CallerContext,
        query: &OwnerSandboxQuery,
    ) -> Result<SandboxInventoryReading, HierarchyReadError> {
        let view = self
            .sandbox_held(held, &held.snapshot.lineage.sandbox.id)
            .await?;
        if !query.matches_status(view.sandbox.status) {
            return Err(authorized::access_error(
                ResourceAccessStoreError::Conflict.into(),
            ));
        }
        Ok(if authorized::personal(held, &context.caller) {
            SandboxInventoryReading::Personal(Box::new(SandboxHierarchySummary {
                sandbox: view.sandbox,
                counts: view.counts,
            }))
        } else {
            SandboxInventoryReading::Shared(view.into())
        })
    }
}
