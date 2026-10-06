//! Management-only encrypted gateway records with optimistic concurrency.
use crate::{Client, Result, http::Request};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// A retained encrypted record. Blob bytes are base64 encoded ciphertext.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GatewayRecord {
    /// Storage identity.
    pub record_id: String,
    /// Owning subject.
    pub owner: String,
    /// Base64 ciphertext.
    pub blob: String,
    /// Current compare-and-swap version.
    pub version: u64,
    /// Initial storage time.
    pub created_at: String,
    /// Most recent storage time.
    pub updated_at: String,
}
/// Record metadata without ciphertext.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GatewayRecordHead {
    /// Storage identity.
    pub record_id: String,
    /// Owning subject.
    pub owner: String,
    /// Current compare-and-swap version.
    pub version: u64,
    /// Most recent storage time.
    pub updated_at: String,
}
/// One bounded page of metadata.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GatewayRecordPage {
    /// Metadata in storage identity order.
    pub items: Vec<GatewayRecordHead>,
    /// Exclusive cursor for the next page.
    pub next_after: Option<String>,
}
/// Create or replace ciphertext with a version precondition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PutGatewayRecord {
    /// Owning subject.
    pub owner: String,
    /// Base64 ciphertext.
    pub blob: String,
    /// None creates; Some replaces only the matching version.
    pub expected_version: Option<u64>,
}
/// Successful storage receipt without ciphertext.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GatewayRecordReceipt {
    /// Storage identity.
    pub record_id: String,
    /// New compare-and-swap version.
    pub version: u64,
    /// Storage time.
    pub updated_at: String,
}
/// Bounded metadata listing options.
#[derive(Debug, Clone, Default)]
pub struct GatewayRecordListOptions {
    /// Optional owner filter.
    pub owner: Option<String>,
    /// Exclusive storage identity cursor.
    pub after: Option<String>,
    /// Page size, at most 200.
    pub limit: Option<u32>,
}
/// Operations requiring the gateway management capability.
pub struct GatewayRecords<'a>(&'a Client, &'static str);
impl Client {
    /// Manage opaque encrypted gateway records.
    pub fn gateway_records(&self) -> GatewayRecords<'_> {
        GatewayRecords(self, "channel-records")
    }
    /// Manage opaque encrypted gateway connection records in their separate namespace.
    pub fn gateway_connection_records(&self) -> GatewayRecords<'_> {
        GatewayRecords(self, "connection-records")
    }
}
impl GatewayRecords<'_> {
    /// Store ciphertext once with its explicit version precondition.
    pub async fn put(&self, id: &str, body: &PutGatewayRecord) -> Result<GatewayRecordReceipt> {
        self.0
            .operation(
                Method::PUT,
                &["gateway", self.1, id],
                Some(body),
                StatusCode::OK,
            )
            .await
    }
    /// Read the full encrypted record.
    pub async fn get(&self, id: &str) -> Result<GatewayRecord> {
        self.0.get(&["gateway", self.1, id], &[]).await
    }
    /// List metadata without downloading ciphertext.
    pub async fn list(&self, options: &GatewayRecordListOptions) -> Result<GatewayRecordPage> {
        let mut query = Vec::new();
        if let Some(value) = &options.owner {
            query.push(("owner", value.clone()));
        }
        if let Some(value) = &options.after {
            query.push(("after", value.clone()));
        }
        if let Some(value) = options.limit {
            query.push(("limit", value.to_string()));
        }
        self.0.get(&["gateway", self.1], &query).await
    }
    /// Delete a record, optionally requiring its current version.
    pub async fn delete(&self, id: &str, expected_version: Option<u64>) -> Result<()> {
        let path = ["gateway", self.1, id];
        let mut request =
            Request::operation::<()>(Method::DELETE, &path, None, StatusCode::NO_CONTENT)?;
        let query = expected_version
            .map(|value| ("expected_version", value.to_string()))
            .into_iter()
            .collect::<Vec<_>>();
        request.query = &query;
        self.0.empty(request).await
    }
}
