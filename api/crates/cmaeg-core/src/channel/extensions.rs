//! Which stored-shape extensions a channel definition uses.
use super::ChannelDefinition;

/// Which extensions a definition uses; an older image cannot read either.
///
/// Readers of an extension ship before any writer of it, so the write gate
/// asks this before storing a definition: a legacy-compatible one is exactly
/// what every deployed image reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extensions {
    /// Optional provider negotiation must be readable before activation.
    pub provider_capabilities: bool,
    /// Optional native automated-group admission, unreadable by older readers.
    pub native_bot_mentions: bool,
    pub lark_resources: bool,
    pub lark_message_effects: bool,
    /// Verified native thread contract, unreadable by older readers.
    pub thread_contract: bool,
    /// Multi-consumer routing.
    pub routing: bool,
    /// Source and partition shapes beyond today's rules.
    pub sources: bool,
}

impl Extensions {
    /// routing: the definition carries a routing policy or consumer keys,
    /// neither of which an older image can parse.
    /// sources: the legacy source and partition rules refuse the definition,
    /// or its partition carries `custom_key`, or a relay opts into raw-event
    /// verification; older readers cannot parse either optional field.
    pub fn of(def: &ChannelDefinition) -> Self {
        Self {
            provider_capabilities: !def.provider.capabilities.is_empty()
                || def.routing.as_ref().is_some_and(|routing| {
                    routing
                        .consumers
                        .iter()
                        .any(|consumer| !consumer.provider.capabilities.is_empty())
                }),
            native_bot_mentions: def.sources.iter().any(super::Source::native_bot_mentions),
            lark_message_effects: def.sources.iter().any(|source| {
                matches!(
                    source,
                    super::Source::NyxidRelay {
                        lark_message_effects: Some(_),
                        ..
                    }
                )
            }),
            lark_resources: def.sources.iter().any(|source| {
                matches!(
                    source,
                    super::Source::NyxidRelay {
                        lark_resources: Some(_),
                        ..
                    }
                )
            }),
            thread_contract: def.reply.thread_contract.is_some(),
            routing: def.routing.is_some() || !def.secrets.consumer_keys.is_empty(),
            sources: def.validate_legacy_sources_and_partition().is_err()
                || def.partition.custom_key.is_some()
                || def.sources.iter().any(|source| {
                    matches!(
                        source,
                        super::Source::NyxidRelay {
                            verify_raw_event: true,
                            ..
                        }
                    )
                }),
        }
    }

    /// Whether an image that predates every extension can read the definition.
    pub fn legacy_compatible(&self) -> bool {
        !self.routing
            && !self.provider_capabilities
            && !self.sources
            && !self.native_bot_mentions
            && !self.thread_contract
            && !self.lark_resources
            && !self.lark_message_effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_provider_capability_requires_a_compatible_rollback_reader() {
        let mut definition: ChannelDefinition =
            serde_json::from_str(include_str!("../../tests/fixtures/channel.json")).unwrap();
        let legacy = serde_json::to_value(&definition).unwrap();
        assert!(legacy["provider"].get("capabilities").is_none());
        assert!(!Extensions::of(&definition).provider_capabilities);
        definition.provider.capabilities = vec!["nyxid_turn_trace_v1".into()];
        let extensions = Extensions::of(&definition);
        assert!(extensions.provider_capabilities);
        assert!(!extensions.legacy_compatible());
        let encoded = serde_json::to_value(&definition).unwrap();
        let retained: ChannelDefinition = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(
            retained.provider.capabilities,
            definition.provider.capabilities
        );
        for invalid in [serde_json::json!(42), serde_json::json!([false])] {
            let mut invalid_definition = encoded.clone();
            invalid_definition["provider"]["capabilities"] = invalid;
            assert!(serde_json::from_value::<ChannelDefinition>(invalid_definition).is_err());
        }
        definition.provider.capabilities.clear();
        assert_eq!(serde_json::to_value(&definition).unwrap(), legacy);
    }
}
