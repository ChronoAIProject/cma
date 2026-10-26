//! The consumer views of a resolved channel, the form turn and tool code use.
use super::resolver::ResolvedChannel;
use cmaeg_core::{channel::ConsumerView, ids::ConsumerId};

impl ResolvedChannel {
    /// The consumer `id` names, `None` meaning the default; see
    /// [`cmaeg_core::channel::ChannelDefinition::consumer`].
    pub fn consumer(&self, id: Option<&ConsumerId>) -> Option<ConsumerView<'_>> {
        self.definition.consumer(id)
    }

    /// Decoded key-hash bytes of a consumer, for constant-time comparison,
    /// `None` meaning the default. Every consumer's were decoded when the
    /// channel was restored; the default's are also kept in `agent_key_hash`,
    /// which stays its authority, so its answer never changes.
    pub fn consumer_key_hash(&self, id: Option<&ConsumerId>) -> Option<&[u8; 32]> {
        match id {
            None => Some(&self.agent_key_hash),
            Some(id) if id.is_default() => Some(&self.agent_key_hash),
            Some(id) => self
                .consumer_keys
                .iter()
                .find(|(consumer, _)| consumer == id)
                .map(|(_, hash)| hash),
        }
    }
}

#[cfg(test)]
#[path = "consumers_tests.rs"]
mod tests;
