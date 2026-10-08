//! The consumer views of a resolved channel, the form turn and tool code use.
use super::resolver::ResolvedChannel;
use cmaeg_core::{channel::ConsumerView, ids::ConsumerId};

impl ResolvedChannel {
    /// The consumer `id` names, `None` meaning the default; see
    /// [`cmaeg_core::channel::ChannelDefinition::consumer`].
    pub fn consumer(&self, id: Option<&ConsumerId>) -> Option<ConsumerView<'_>> {
        self.definition.consumer(id)
    }

    /// Decoded key-hash bytes of a consumer, for constant-time comparison.
    /// The default consumer's were decoded when the channel was restored.
    pub fn consumer_key_hash(&self, id: Option<&ConsumerId>) -> Option<&[u8; 32]> {
        self.consumer(id).map(|_| &self.agent_key_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmaeg_core::{channel::ChannelDefinition, ids::ChannelId, seal::ChannelPointer};

    fn channel() -> ResolvedChannel {
        let definition: ChannelDefinition = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        ResolvedChannel {
            id: ChannelId::new("cmaeg1.ch.consumers-fixture").unwrap(),
            pointer: ChannelPointer {
                v: 1,
                record_id: definition.record_id.clone(),
                record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
            },
            definition,
            agent_key_hash: [7; 32],
            loaded_at: std::time::Instant::now(),
            version: 1,
        }
    }

    #[test]
    fn the_default_consumer_resolves_with_its_restored_key_hash() {
        let channel = channel();
        let default = ConsumerId::default_consumer();
        for id in [None, Some(&default)] {
            let view = channel.consumer(id).unwrap();
            assert!(view.id.is_default());
            assert_eq!(view.provider.binding_id.as_str(), "binding-1");
            assert_eq!(channel.consumer_key_hash(id), Some(&[7; 32]));
        }
    }

    #[test]
    fn an_unknown_consumer_has_no_view_and_no_key_hash() {
        let channel = channel();
        let other = ConsumerId::parse("csm_0123456789ab").unwrap();
        assert!(channel.consumer(Some(&other)).is_none());
        assert!(channel.consumer_key_hash(Some(&other)).is_none());
    }
}
