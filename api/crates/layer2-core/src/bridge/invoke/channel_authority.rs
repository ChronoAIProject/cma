use crate::resource_access::CallerContext;

use super::BridgeInvokeError;

/// Trusted process-only renewable human authority. Implementations retain the
/// actual credential lease; this proof is never accepted from a public DTO or
/// serialized into an invocation grant. Storage compares the exact durable
/// grant digest as well as selected-key revision in the grant transaction.
pub trait ChannelInvokeAuthority: Send + Sync {
    fn actor_context(&self) -> &CallerContext;
    fn credential(&self) -> &str;
    fn deadline(&self) -> tokio::time::Instant;
    fn custody_revision(&self) -> u64;
    fn control_plane_digest(&self) -> Result<String, BridgeInvokeError>;

    fn require_current(&self) -> Result<(), BridgeInvokeError> {
        if tokio::time::Instant::now() >= self.deadline() || self.custody_revision() == 0 {
            return Err(BridgeInvokeError::Unavailable);
        }
        Ok(())
    }
}
