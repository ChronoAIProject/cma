//! Capture only authority already held by this process.

use super::{
    super::authority::ReceiverCancellation, Authority, OwnershipCoordinator, OwnershipToken,
};
use crate::qualification::agent_digest;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// The original local authority. This handle cannot be retargeted or reused.
pub struct CapturedOwner(Arc<Authority>);

impl CapturedOwner {
    pub fn agent_digest(&self) -> [u8; 32] {
        agent_digest(&self.0.token.agent_id)
    }

    pub fn matches(&self, authority: &Arc<Authority>) -> bool {
        Arc::ptr_eq(&self.0, authority) && self.0.is_current()
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

impl OwnershipCoordinator {
    pub async fn capture_qualification_owner_digest(
        &self,
        selected_agent: [u8; 32],
        selected_owner: [u8; 32],
    ) -> Option<CapturedOwner> {
        if !self.is_ready() {
            return None;
        }
        let local = self.owned.lock().await;
        if !self.is_ready() {
            return None;
        }
        local
            .values()
            .find(|authority| {
                authority.token.instance_id == self.instance_id
                    && authority.is_current()
                    && agent_digest(&authority.token.agent_id) == selected_agent
                    && qualification_owner_digest(&authority.token) == selected_owner
            })
            .map(|authority| CapturedOwner(Arc::clone(authority)))
    }

    pub fn attach_qualification_renewal(
        &self,
        hook: Arc<dyn crate::qualification::RenewalHook>,
    ) -> bool {
        self.qualification_renewal.set(hook).is_ok()
    }
}

/// Encode the current ownership token without exporting any of its raw fields.
pub fn qualification_owner_digest(token: &OwnershipToken) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"cma-qualification/owner/v1\0");
    digest.update(agent_digest(&token.agent_id));
    digest.update((token.instance_id.len() as u64).to_be_bytes());
    digest.update(token.instance_id.as_bytes());
    digest.update(token.epoch.to_be_bytes());
    digest.update(token.protocol.to_be_bytes());
    digest.finalize().into()
}
