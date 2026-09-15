mod types;
mod validation;
use super::{Bridges, validation as bridge_validation};
use crate::{Error, IdempotencyKey, Result};
use reqwest::{Method, StatusCode};
pub use types::*;

impl Bridges<'_> {
    /// Read retained acceptance and current progress without admitting cleanup or reviving a parent.
    pub async fn read_workspace_cleanup(
        &self,
        bridge_id: &str,
        allocation_id: &str,
    ) -> Result<BridgeWorkspaceCleanupStatus> {
        bridge_validation::id(bridge_id)?;
        validation::allocation_id(allocation_id)?;
        let status: BridgeWorkspaceCleanupStatus = self
            .client
            .get(
                &[
                    "bridges",
                    bridge_id,
                    "allocations",
                    allocation_id,
                    "workspace-cleanup",
                ],
                &[],
            )
            .await?;
        status.check(bridge_id, allocation_id, None)?;
        Ok(status)
    }
    /// Explicitly submit once, requiring 202 acceptance. Preserve this exact request and
    /// key after cancellation or response loss; an identical replay recovers its receipt.
    pub async fn request_workspace_cleanup(
        &self,
        bridge_id: &str,
        allocation_id: &str,
        body: &BridgeWorkspaceCleanupRequest,
        key: &IdempotencyKey,
    ) -> Result<BridgeWorkspaceCleanupStatus> {
        bridge_validation::id(bridge_id)?;
        validation::allocation_id(allocation_id)?;
        validation::request(body)?;
        if key.as_str().len() > 200 {
            return Err(Error::validation(
                "Workspace cleanup idempotency key exceeds 200 characters",
            ));
        }
        let status: BridgeWorkspaceCleanupStatus = self
            .client
            .command(
                Method::POST,
                &[
                    "bridges",
                    bridge_id,
                    "allocations",
                    allocation_id,
                    "workspace-cleanup",
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
