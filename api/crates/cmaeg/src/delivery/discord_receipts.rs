//! Reaction target custody is opted in per native source and deployed reader.
use super::send::Sender;
use cmaeg_core::{channel::Source, seal::EventRefPayload};
impl Sender {
    pub(super) async fn discord_reaction_receipt(
        &self,
        channel: &crate::channels::resolver::ResolvedChannel,
        event: &EventRefPayload,
    ) -> Result<Option<crate::discord::receipts::Prepared>, crate::discord::GatewayError> {
        if !self.relay.discord_reactions_enabled() || !channel.definition.sources.iter().any(|source| {
            matches!(source, Source::Discord { intents, .. } if intents & crate::discord::REACTION_INTENTS != 0)
        }) { return Ok(None); }
        let store = self
            .relay
            .interactions
            .as_ref()
            .ok_or(crate::discord::GatewayError::Unavailable)?;
        store.discord_receipt(channel, event).await.map(Some)
    }
}
