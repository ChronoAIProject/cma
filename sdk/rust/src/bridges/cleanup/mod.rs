mod types;
mod validation;
use super::{Bridges, validation as bridge_validation};
use crate::{Error, IdempotencyKey, PageOptions, Result};
use reqwest::{Method, StatusCode};
pub use types::*;

impl Bridges<'_> {
    /// List exact allocations under current Bridge and whole-parent management authority.
    /// Keep opaque cursors unchanged and scoped to the same Bridge.
    pub async fn list_allocations(
        &self,
        bridge_id: &str,
        options: &PageOptions,
    ) -> Result<BridgeCleanupAllocationPage> {
        bridge_validation::id(bridge_id)?;
        let limit = options.limit.unwrap_or(50);
        if limit == 0 || limit > 100 {
            return Err(Error::validation("allocation limit must be 1 through 100"));
        }
        if options
            .cursor
            .as_deref()
            .is_some_and(|cursor| !validation::cursor(cursor))
        {
            return Err(Error::validation("invalid opaque allocation cursor"));
        }
        let page: BridgeCleanupAllocationPage = self
            .client
            .get(&["bridges", bridge_id, "allocations"], &options.query())
            .await?;
        page.check(bridge_id, options)?;
        Ok(page)
    }
    /// Read retained acceptance and current progress without admitting cleanup or reviving a parent.
    pub async fn read_child_cleanup(
        &self,
        bridge_id: &str,
        allocation_id: &str,
    ) -> Result<BridgeChildCleanupStatus> {
        bridge_validation::id(bridge_id)?;
        validation::allocation_id(allocation_id)?;
        let status: BridgeChildCleanupStatus = self
            .client
            .get(
                &[
                    "bridges",
                    bridge_id,
                    "allocations",
                    allocation_id,
                    "child-cleanup",
                ],
                &[],
            )
            .await?;
        status.check(bridge_id, allocation_id, None)?;
        Ok(status)
    }
    /// Explicitly submit once, requiring 202 acceptance. Preserve this exact request and
    /// key after cancellation or response loss; an identical replay recovers its receipt.
    pub async fn request_child_cleanup(
        &self,
        bridge_id: &str,
        allocation_id: &str,
        body: &BridgeChildCleanupRequest,
        key: &IdempotencyKey,
    ) -> Result<BridgeChildCleanupStatus> {
        bridge_validation::id(bridge_id)?;
        validation::allocation_id(allocation_id)?;
        bridge_validation::revision(body.expected_bridge_revision)?;
        bridge_validation::revision(body.expected_allocation_revision)?;
        if key.as_str().len() > 200 {
            return Err(Error::validation(
                "child cleanup idempotency key exceeds 200 characters",
            ));
        }
        let status: BridgeChildCleanupStatus = self
            .client
            .command(
                Method::POST,
                &[
                    "bridges",
                    bridge_id,
                    "allocations",
                    allocation_id,
                    "child-cleanup",
                ],
                Some(body),
                key,
                StatusCode::ACCEPTED,
            )
            .await?;
        status.check(bridge_id, allocation_id, Some(body))?;
        Ok(status)
    }
}
