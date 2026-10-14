//! Which stored-shape extensions a channel definition uses.
use super::ChannelDefinition;

/// Which extensions a definition uses; an older image cannot read either.
///
/// Readers of an extension ship before any writer of it, so the write gate
/// asks this before storing a definition: a legacy-compatible one is exactly
/// what every deployed image reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extensions {
    /// Multi-consumer routing.
    pub routing: bool,
    /// Source and partition shapes beyond today's rules.
    pub sources: bool,
}

impl Extensions {
    /// routing: the definition carries a routing policy or consumer keys,
    /// neither of which an older image can parse.
    /// sources: the legacy source and partition rules refuse the definition,
    /// or its partition carries `custom_key`, which an older image cannot parse.
    pub fn of(def: &ChannelDefinition) -> Self {
        Self {
            routing: def.routing.is_some() || !def.secrets.consumer_keys.is_empty(),
            sources: def.validate_legacy_sources_and_partition().is_err()
                || def.partition.custom_key.is_some(),
        }
    }

    /// Whether an image that predates every extension can read the definition.
    pub fn legacy_compatible(&self) -> bool {
        !self.routing && !self.sources
    }
}
