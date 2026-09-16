use super::super::BridgeInvokeError;
use super::{SourceControlAccess, SourceControlPolicy, SourceControlReceipt, SourceControlWrite};

#[async_trait::async_trait]
pub trait BridgeSourceControlStore: Send + Sync {
    /// Read current policy and permanent audit/receipt proof in one snapshot.
    /// A missing head with retained history is corrupt, never default permission.
    async fn source_control_policy(
        &self,
        access: &SourceControlAccess,
    ) -> Result<Option<SourceControlPolicy>, BridgeInvokeError>;
    /// Exact receipt recovery precedes revision and current eligibility checks;
    /// current access and immutable target identity still apply.
    async fn recover_source_control(
        &self,
        access: &SourceControlAccess,
        receipt_id: &str,
        request_digest: &str,
    ) -> Result<Option<SourceControlReceipt>, BridgeInvokeError>;
    /// Commit current Bridge/grant/target fences, policy CAS, immutable audit and
    /// receipt together. This grants no runtime effect or old-grant migration.
    async fn apply_source_control(
        &self,
        write: SourceControlWrite,
    ) -> Result<SourceControlReceipt, BridgeInvokeError>;
}
