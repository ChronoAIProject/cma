//! Claim discovery retains pages; malformed pages never advance the cursor.
use super::{
    claims, cleanup_inventory::PageCursor, model, rejected, AgentSandboxPools, NativePoolClaim,
    NativePoolClaimRequest, Result, OWNER_LABEL,
};
use crate::InventoryCursor;
use kube::ResourceExt;

pub(super) struct Cursor {
    pools: AgentSandboxPools,
    pages: PageCursor,
    items: Vec<NativePoolClaim>,
}

impl Cursor {
    pub(super) fn new(pools: AgentSandboxPools) -> Self {
        Self {
            pools,
            pages: PageCursor::default(),
            items: vec![],
        }
    }
}

#[async_trait::async_trait]
impl InventoryCursor<NativePoolClaim> for Cursor {
    async fn advance(&mut self) -> Result<Option<Vec<NativePoolClaim>>> {
        let page =
            crate::inventory_cursor::request(crate::inventory_cursor::Child::RouteList, async {
                self.pools
                    .claims
                    .list(&self.pages.params())
                    .await
                    .map_err(super::api_error)
            })
            .await?;
        let mut items = Vec::new();
        for claim in &page.items {
            if claim.labels().get(OWNER_LABEL) != Some(&self.pools.config.deployment) {
                continue;
            }
            let encoded = claim
                .annotations()
                .get("cma/pool-claim-request")
                .filter(|v| v.len() <= 128 * 1024)
                .ok_or_else(|| rejected("native claim has no allocation request"))?;
            let request: NativePoolClaimRequest = serde_json::from_str(encoded)
                .map_err(|_| rejected("native claim allocation request is invalid"))?;
            let mut claim = claim.clone();
            claim.metadata.deletion_timestamp = None;
            claims::verify_claim(&claim, &model::claim(&self.pools.config, &request)?)?;
            items.push(claims::receipt(&request, &claim)?);
        }
        let complete = self.pages.accept(&page)?;
        self.items.extend(items);
        Ok(complete.then(|| std::mem::take(&mut self.items)))
    }
}
