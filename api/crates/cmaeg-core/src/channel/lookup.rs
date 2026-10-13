//! Per-event source lookups. A channel may hold several chat sources, so
//! code acting on one activity or message finds that activity's own source
//! instead of the first source of a kind.
use super::{ChannelDefinition, Extensions, Source};
use crate::activity::SourceKind;

impl ChannelDefinition {
    /// The source an activity arrived on: Telegram{bot_id} → the native source with that
    /// bot_id; NyxidRelay{platform,..} → the relay with that platform; Custom → the custom source.
    pub fn source_of(&self, kind: &SourceKind) -> Option<&Source> {
        self.sources.iter().find(|source| match (kind, source) {
            (
                SourceKind::Telegram { bot_id, .. },
                Source::Telegram {
                    bot_id: configured, ..
                },
            ) => bot_id == configured,
            (
                SourceKind::NyxidRelay { platform, .. },
                Source::NyxidRelay {
                    platform: configured,
                    ..
                },
            ) => platform == configured,
            (SourceKind::Custom, Source::Custom { .. }) => true,
            _ => false,
        })
    }

    /// The relay whose route_ids contain `route_id` (unique by the disjointness rule).
    pub fn relay_source_for_route(&self, route_id: &str) -> Option<&Source> {
        self.sources.iter().find(|source| {
            matches!(source, Source::NyxidRelay { route_ids, .. }
                if route_ids.iter().any(|route| route == route_id))
        })
    }

    /// The bot username of the source an activity arrived on.
    pub fn bot_username_of(&self, kind: &SourceKind) -> Option<&str> {
        match self.source_of(kind)? {
            Source::Telegram { bot_username, .. } | Source::NyxidRelay { bot_username, .. } => {
                bot_username.as_deref()
            }
            Source::Custom { .. } => None,
        }
    }

    /// Whether the definition needs the extended readers. Code with a
    /// legacy, first-of-a-kind behaviour keeps it exactly while this is false.
    pub fn has_extended_sources(&self) -> bool {
        Extensions::of(self).sources
    }
}
