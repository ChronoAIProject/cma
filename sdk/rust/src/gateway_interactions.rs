//! Expiring opaque interaction records for gateway service authority.
use crate::{
    Client, Result,
    gateway_records::{GatewayRecord, GatewayRecordReceipt},
    http::Request,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// Create or replace an encrypted interaction with a version precondition.
#[derive(Clone, Serialize, Deserialize)]
pub struct PutGatewayInteraction {
    /// Owning subject.
    pub owner: String,
    /// Base64 ciphertext; CMA does not interpret it.
    pub blob: String,
    /// None creates; Some requires the matching current version.
    pub expected_version: Option<u64>,
    /// Expiry as Unix seconds, within the server's retention limit.
    pub expires_at: i64,
}
/// Operations requiring the gateway management capability.
pub struct GatewayInteractions<'a>(&'a Client);
impl Client {
    /// Manage expiring encrypted gateway interaction state.
    pub fn gateway_interactions(&self) -> GatewayInteractions<'_> {
        GatewayInteractions(self)
    }
}
impl GatewayInteractions<'_> {
    /// Store ciphertext with an explicit version precondition and expiry.
    pub async fn put(
        &self,
        id: &str,
        body: &PutGatewayInteraction,
    ) -> Result<GatewayRecordReceipt> {
        self.0
            .operation(
                Method::PUT,
                &["gateway", "interaction-records", id],
                Some(body),
                StatusCode::OK,
            )
            .await
    }
    /// Read an unexpired encrypted record.
    pub async fn get(&self, id: &str) -> Result<GatewayRecord> {
        self.0
            .get(&["gateway", "interaction-records", id], &[])
            .await
    }
    /// Delete only the exact observed version.
    pub async fn delete(&self, id: &str, expected_version: u64) -> Result<()> {
        let path = ["gateway", "interaction-records", id];
        let mut request =
            Request::operation::<()>(Method::DELETE, &path, None, StatusCode::NO_CONTENT)?;
        let query = [("expected_version", expected_version.to_string())];
        request.query = &query;
        self.0.empty(request).await
    }
}
