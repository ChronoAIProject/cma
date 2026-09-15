//! Separate original-sender consent. Management does not execute runtime controls.
mod types;
mod validation;
use super::Bridges;
use crate::{Error, IdempotencyKey, Result};
use reqwest::{Method, StatusCode};
pub use types::*;

impl Bridges<'_> {
    /// Read current consent; None is distinct from a recorded disabled policy.
    pub async fn read_source_controls(&self, grant_id: &str) -> Result<BridgeSourceControlState> {
        validation::id(grant_id)?;
        let state: BridgeSourceControlState = self
            .client
            .get(&["bridge-invoke-grants", grant_id, "source-controls"], &[])
            .await?;
        if let Some(policy) = &state.policy {
            policy.check(grant_id)?;
        }
        Ok(state)
    }
    /// Submit once with exact observed revisions. Reusing the original request/key
    /// recovers its immutable receipt after response loss, even following revocation.
    /// A recovered enabled receipt does not re-enable the current policy.
    pub async fn set_source_controls(
        &self,
        grant_id: &str,
        body: &BridgeSourceControlRequest,
        key: &IdempotencyKey,
    ) -> Result<BridgeSourceControlReceipt> {
        validation::id(grant_id)?;
        body.check().map_err(Error::validation)?;
        if key.as_str().len() > 200 {
            return Err(Error::validation(
                "source-control consent key exceeds200 characters",
            ));
        }
        let receipt: BridgeSourceControlReceipt = self
            .client
            .command(
                Method::PUT,
                &["bridge-invoke-grants", grant_id, "source-controls"],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await?;
        receipt.check(grant_id, body)?;
        Ok(receipt)
    }
}
