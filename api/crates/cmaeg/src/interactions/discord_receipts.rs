//! Delivery may prepare custody; only a confirmed Discord response binds a message ID.
use super::InteractionStore;
impl InteractionStore {
    pub(crate) async fn discord_receipt(
        &self,
        channel: &crate::channels::resolver::ResolvedChannel,
        event: &cmaeg_core::seal::EventRefPayload,
    ) -> Result<crate::discord::receipts::Prepared, crate::discord::GatewayError> {
        crate::discord::receipts::Prepared::new(
            self.records.clone(),
            self.keys.clone(),
            channel,
            event,
        )
        .await
    }
}
