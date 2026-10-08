//! The agents that consume a channel's events, as downstream code sees them.
use std::sync::OnceLock;

use super::{ChannelDefinition, ProviderBinding, Secret};
use crate::{ids::ConsumerId, profile::EventWorkerProfile};

/// The one view of "which agent serves this turn" that turn, tool, context,
/// card and media code may use. Downstream code never reads the top-level
/// provider fields directly, so adding consumers needs no caller change.
///
/// Only the implicit `default` consumer exists for now, backed by the
/// definition's unchanged top-level storage. Multi-consumer routing (#988)
/// adds the additional consumers, and with them `mentionable`, `non_human`,
/// the `reply` destination, `publish` and `pending`.
#[derive(Debug, Clone, Copy)]
pub struct ConsumerView<'a> {
    /// Borrowed from a static for `default`, or from the routing policy.
    pub id: &'a ConsumerId,
    /// `default` until routing exists.
    pub name: &'a str,
    /// Equal to `name` until routing exists.
    pub label: &'a str,
    pub profile: &'a EventWorkerProfile,
    pub profile_digest: &'a str,
    pub provider: &'a ProviderBinding,
    /// The public hash of the consumer's agent key, `sha256:<64 hex>`.
    pub agent_key_hash: &'a str,
    /// The consumer's agent key, used as its provider credential.
    pub credential: Option<&'a Secret>,
    /// Effective automatic finalization: today the channel's `reply.auto_final`.
    pub auto_final: bool,
}

/// The `default` id, built once so every default view can borrow it.
fn default_id() -> &'static ConsumerId {
    static DEFAULT: OnceLock<ConsumerId> = OnceLock::new();
    DEFAULT.get_or_init(ConsumerId::default_consumer)
}

impl ChannelDefinition {
    /// `None` or `Some("default")` returns the implicit default consumer.
    /// Any other id is `None` until additional consumers exist.
    pub fn consumer(&self, id: Option<&ConsumerId>) -> Option<ConsumerView<'_>> {
        match id {
            None => Some(self.default_consumer()),
            Some(id) if id.is_default() => Some(self.default_consumer()),
            Some(_) => None,
        }
    }

    /// The default consumer first; only it exists for now.
    pub fn consumers(&self) -> impl Iterator<Item = ConsumerView<'_>> {
        std::iter::once(self.default_consumer())
    }

    fn default_consumer(&self) -> ConsumerView<'_> {
        let id = default_id();
        ConsumerView {
            id,
            name: id.as_str(),
            label: id.as_str(),
            profile: &self.profile,
            profile_digest: &self.profile_digest,
            provider: &self.provider,
            agent_key_hash: &self.secrets.agent_key_hash,
            credential: self.secrets.provider_credential.as_ref(),
            auto_final: self.reply.auto_final,
        }
    }
}
