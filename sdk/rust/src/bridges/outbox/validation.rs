use super::*;
fn prefixed(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 64
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
pub(super) fn item_id(value: &str) -> Result<()> {
    if prefixed(value, "bro_") {
        Ok(())
    } else {
        Err(Error::validation("invalid delivery identity"))
    }
}
impl BridgeDelivery {
    pub(super) fn check(&self, bridge_id: &str) -> Result<()> {
        if !prefixed(&self.id, "bro_")
            || self.bridge_id != bridge_id
            || !prefixed(&self.receipt_id, "brti_")
            || !super::super::validation::valid_id(&self.response_id)
            || !(1..=256).contains(&self.config_revision)
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || self.sequence == 0
            || self.sequence > i64::MAX as u64
            || !(1..=1024).contains(&self.chunk_count)
            || self.chunk_index >= self.chunk_count
            || self.updated_at_ms < self.created_at_ms
            || self.updated_at_ms > i64::MAX as u64
            || self.attempts.len() > 16
            || ((self.status == BridgeDeliveryStatus::RetryWait) != self.next_retry_at_ms.is_some())
            || self
                .next_retry_at_ms
                .is_some_and(|at| at <= self.updated_at_ms || at > i64::MAX as u64)
            || self.attempts.iter().enumerate().any(|(index, attempt)| {
                !prefixed(&attempt.id, "broa_")
                    || attempt.number as usize != index + 1
                    || attempt.started_at_ms < self.created_at_ms
                    || attempt.started_at_ms > self.updated_at_ms
            })
        {
            return Err(Error::protocol(
                "delivery identity, attempt history or retry bounds are inconsistent",
            ));
        }
        Ok(())
    }
}
impl BridgeDeliveryRecoveryReceipt {
    pub(super) fn check(
        &self,
        bridge_id: &str,
        request: &BridgeDeliveryRecoveryRequest,
    ) -> Result<()> {
        self.result.check(bridge_id)?;
        let revision_matches = request.expected_revision.checked_add(1)
            == Some(self.result.revision)
            || (matches!(request.action, BridgeDeliveryRecoveryAction::Reconcile {})
                && request.expected_revision == self.result.revision);
        if !prefixed(&self.id, "brrc_")
            || !prefixed(&self.request_sha256, "")
            || self.bridge_id != bridge_id
            || self.result.id != request.item_id
            || self.action != request.action
            || !revision_matches
            || self.created_at_ms < self.result.updated_at_ms
            || self.created_at_ms > i64::MAX as u64
            || !matches!(&self.actor, crate::resource_access::ResourceActor::Human {subject} | crate::resource_access::ResourceActor::Api {subject,delegation_id:None} if super::super::validation::valid_id(subject))
            || (matches!(request.action, BridgeDeliveryRecoveryAction::Retry { .. })
                && self.result.status != BridgeDeliveryStatus::Pending)
            || (matches!(request.action, BridgeDeliveryRecoveryAction::Abandon {})
                && self.result.status != BridgeDeliveryStatus::Abandoned)
        {
            return Err(Error::protocol(
                "delivery recovery receipt does not match its command",
            ));
        }
        Ok(())
    }
}
