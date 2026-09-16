use super::*;
use crate::resource_access::ResourceActor;
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub(super) fn allocation_id(value: &str) -> bool {
    value.strip_prefix("bral_").is_some_and(digest)
}
impl BridgePartitionCommandReceipt {
    pub(super) fn check(
        &self,
        bridge_id: &str,
        allocation: &str,
        body: &BridgePartitionCommandRequest,
        action: BridgePartitionCommandAction,
    ) -> Result<()> {
        if !self.id.strip_prefix("bpc_").is_some_and(digest)
            || !digest(&self.request_sha256)
            || self.bridge_id != bridge_id
            || self.allocation_id != allocation
            || !allocation_id(allocation)
            || self.action != action
            || self.expected_bridge_revision != body.expected_bridge_revision
            || self.previous_revision != body.expected_revision
            || !(1..=256).contains(&self.generation)
            || self.result.id != allocation
            || self.result.generation != self.generation
            || Some(self.result.revision) != self.previous_revision.checked_add(1)
            || self.result.revision > i64::MAX as u64
            || self.result.state != BridgePartitionCommandState::Retired
            || self.created_at_ms > i64::MAX as u64
            || !matches!(&self.actor, ResourceActor::Human { subject } | ResourceActor::Api { subject, delegation_id: None } if bridge::valid_id(subject))
        {
            return Err(Error::protocol(
                "partition receipt does not match the exact command",
            ));
        }
        match (action, &self.successor) {
            (BridgePartitionCommandAction::Detach, None) => {}
            (BridgePartitionCommandAction::Reset, Some(next))
                if allocation_id(&next.id)
                    && next.id != allocation
                    && next.generation == self.generation + 1
                    && next.generation <= 256
                    && next.revision == 1
                    && matches!(
                        next.state,
                        BridgePartitionCommandState::Reserved | BridgePartitionCommandState::Ready
                    ) => {}
            _ => {
                return Err(Error::protocol(
                    "partition receipt has an inconsistent successor",
                ));
            }
        }
        Ok(())
    }
}
