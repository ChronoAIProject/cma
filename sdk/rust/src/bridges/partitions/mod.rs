mod pending;
mod types;
mod validation;
use super::{Bridges, validation as bridge};
use crate::{Error, PageOptions, Result};
pub use pending::*;
pub use types::*;

impl Bridges<'_> {
    /// List exact allocation generations under current Bridge and resource
    /// authority. Preserve the opaque, Bridge-scoped cursor; maximum 25 rows.
    pub async fn list_partitions(
        &self,
        bridge_id: &str,
        options: &PageOptions,
    ) -> Result<BridgePartitionPage> {
        bridge::id(bridge_id)?;
        let limit = options.limit.unwrap_or(25);
        if !(1..=25).contains(&limit) {
            return Err(Error::validation("partition limit must be 1 through 25"));
        }
        let after = options
            .cursor
            .as_deref()
            .map(|cursor| validation::cursor_after(cursor, bridge_id))
            .transpose()
            .map_err(|_| Error::validation("invalid partition cursor for this Bridge"))?;
        let page: BridgePartitionPage = self
            .client
            .get(&["bridges", bridge_id, "partitions"], &options.query())
            .await?;
        page.check(bridge_id, after.as_deref(), usize::from(limit))?;
        Ok(page)
    }

    /// Read at most 64 pending Channel commands and their latest response.
    /// This never resets, detaches, revives, retries or sends a command.
    pub async fn read_partition(
        &self,
        bridge_id: &str,
        allocation_id: &str,
    ) -> Result<BridgePartitionDetail> {
        bridge::id(bridge_id)?;
        if !validation::allocation_id(allocation_id) {
            return Err(Error::validation("invalid allocation identity"));
        }
        let detail: BridgePartitionDetail = self
            .client
            .get(&["bridges", bridge_id, "partitions", allocation_id], &[])
            .await?;
        detail.check(bridge_id, allocation_id)?;
        Ok(detail)
    }
}
