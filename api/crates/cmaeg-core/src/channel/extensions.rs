//! Which stored-shape extensions a channel definition uses.
use super::ChannelDefinition;

/// Which extensions a definition uses; an older image cannot read either.
///
/// Readers of an extension ship before any writer of it, so the write gate
/// asks this before storing a definition: a legacy-compatible one is exactly
/// what every deployed image reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extensions {
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
            && !self.sources
            && !self.native_bot_mentions
            && !self.thread_contract
            && !self.lark_resources
            && !self.lark_message_effects
    }
}
