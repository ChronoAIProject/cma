//! Capture only authority already held by this process.

use super::{
    super::authority::ReceiverCancellation, Authority, OwnershipCoordinator, OwnershipToken,
};
use crate::qualification::agent_digest;
use std::sync::Arc;

/// The original local authority. This handle cannot be retargeted or reused.
pub struct CapturedOwner(Arc<Authority>);

impl CapturedOwner {
    pub fn agent_digest(&self) -> [u8; 32] {
        agent_digest(&self.0.token.agent_id)
    }

    pub fn cancel(self) -> bool {
        self.0.cancel_receiver() == ReceiverCancellation::CancelledWhileLive
    }
}

impl OwnershipCoordinator {
    pub async fn capture_qualification_owner(
        &self,
        token: &OwnershipToken,
    ) -> Option<CapturedOwner> {
        if token.instance_id != self.instance_id || !self.is_ready() {
            return None;
        }
        let local = self.owned.lock().await;
        let authority = local.get(&token.agent_id)?;
        (self.is_ready() && authority.token == *token && authority.is_current())
            .then(|| CapturedOwner(Arc::clone(authority)))
    }
}
