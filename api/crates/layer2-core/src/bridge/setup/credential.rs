use async_trait::async_trait;
use serde::Serialize;
use zeroize::Zeroizing;

use super::super::{BridgeError, BridgeId, BridgePlatform, BridgeService};
use crate::resource_access::{CallerContext, ResourceOwner};

/// Write-only selected key material. Debug and responses never reveal it.
pub struct BridgeKeyMaterial(Zeroizing<String>);
impl BridgeKeyMaterial {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for BridgeKeyMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BridgeKeyMaterial([redacted])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BridgeCredentialStatus {
    pub revision: u64,
    pub present: bool,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BridgeBotChoice {
    pub id: String,
    pub platform: BridgePlatform,
    pub active: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct BridgeKeyChoice {
    pub id: String,
    pub active: bool,
    pub callback_configured: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct BridgeRouteChoice {
    pub id: String,
    pub bot_id: Option<String>,
    pub key_id: String,
    pub platform: String,
    pub active: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct BridgeConnectorChoices {
    pub bots: Vec<BridgeBotChoice>,
    pub keys: Vec<BridgeKeyChoice>,
    pub routes: Vec<BridgeRouteChoice>,
}

pub struct BridgeCredentialImport {
    pub id: BridgeId,
    pub expected_revision: u64,
    pub material: BridgeKeyMaterial,
    /// Explicit permission to use this exact sign-in for this Bridge for 30 days.
    pub retain_control_plane_authority: bool,
}
pub struct BridgeCredentialRevocation {
    pub id: BridgeId,
    pub expected_revision: u64,
    pub expected_custody_revision: u64,
    pub idempotency_key: String,
}

/// Implementations use only the explicit current caller for owner-authorized
/// discovery, and only the selected key for durable control-plane custody.
/// These ports do not configure NyxID, send platform messages or mint runtimes.
#[async_trait]
pub trait BridgeSetupCredentials: Send + Sync {
    async fn choices(
        &self,
        context: &CallerContext,
        credential: &str,
        owner: &ResourceOwner,
    ) -> Result<BridgeConnectorChoices, BridgeError>;
    async fn status(
        &self,
        service: &BridgeService,
        context: &CallerContext,
        credential: Option<&str>,
        id: &BridgeId,
        expected_revision: u64,
    ) -> Result<BridgeCredentialStatus, BridgeError>;
    async fn import(
        &self,
        service: &BridgeService,
        context: &CallerContext,
        credential: Option<&str>,
        request: BridgeCredentialImport,
    ) -> Result<BridgeCredentialStatus, BridgeError>;
    async fn revoke(
        &self,
        service: &BridgeService,
        context: &CallerContext,
        credential: Option<&str>,
        request: BridgeCredentialRevocation,
    ) -> Result<BridgeCredentialStatus, BridgeError>;
}
