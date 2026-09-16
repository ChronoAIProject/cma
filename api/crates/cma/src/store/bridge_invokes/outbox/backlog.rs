//! Count retained nonterminal items under the existing Bridge write fence.
use super::*;

impl MongoBridgeReplyToolStore {
    /// Fixed deployment bound, shared by all inputs and generations of a Bridge.
    pub(crate) const MAX_OUTSTANDING_ITEMS: u64 = 256;

    /// The caller already writes the exact Bridge definition in this transaction
    /// through fence_turn or fence_manager. Every admission/recovery increasing
    /// occupancy therefore conflicts with competing writers across replicas.
    /// Terminal delivery transitions only reduce occupancy, so a concurrent
    /// completion can conservatively delay admission but cannot overfill it.
    pub(super) async fn require_backlog_capacity(
        &self,
        source: &OutboxSource,
        additional: usize,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let additional = u64::try_from(additional).map_err(|_| OutboxError::Capacity)?;
        if additional > Self::MAX_OUTSTANDING_ITEMS {
            return Err(OutboxError::Capacity.into());
        }
        let outstanding = self
            .items
            .count_documents(doc! {
                "record.source.pin.bridge_id":source.pin().bridge_id.as_str(),
                // Unknown delivery remains occupied. Missing/unrecognized indexed
                // states are also counted instead of silently granting capacity.
                "state":{"$nin":["sent","rejected","abandoned"]},
            })
            .limit(Self::MAX_OUTSTANDING_ITEMS + 1)
            .hint(mongodb::options::Hint::Name(
                "bridge_outbox_backlog_v1".into(),
            ))
            .session(session)
            .await?;
        if outstanding.saturating_add(additional) > Self::MAX_OUTSTANDING_ITEMS {
            return Err(OutboxError::Capacity.into());
        }
        Ok(())
    }
}

pub(super) fn occupies_capacity(state: &DeliveryState) -> bool {
    !matches!(
        state,
        DeliveryState::Sent { .. }
            | DeliveryState::Rejected { .. }
            | DeliveryState::Abandoned { .. }
    )
}
