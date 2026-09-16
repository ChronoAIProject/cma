use layer2_core::bridge::invoke::{BridgeInvokeError, ChannelInvokeAuthority};

use super::{BridgeControlPlaneGrant, BridgeControlPlaneLease};

pub(crate) fn grant_digest(grant: &BridgeControlPlaneGrant) -> Result<String, BridgeInvokeError> {
    use sha2::{Digest, Sha256};
    grant.validate().map_err(|_| BridgeInvokeError::Corrupt)?;
    let bytes = serde_json::to_vec(&("cma.channel.control-plane.v1", grant))
        .map_err(|_| BridgeInvokeError::Corrupt)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

impl ChannelInvokeAuthority for BridgeControlPlaneLease {
    fn actor_context(&self) -> &layer2_core::resource_access::CallerContext {
        self.actor_context()
    }
    fn credential(&self) -> &str {
        self.credential()
    }
    fn deadline(&self) -> tokio::time::Instant {
        self.deadline()
    }
    fn custody_revision(&self) -> u64 {
        self.custody_revision()
    }
    fn control_plane_digest(&self) -> Result<String, BridgeInvokeError> {
        grant_digest(self.grant())
    }
}
