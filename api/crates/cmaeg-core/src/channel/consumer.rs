//! The agents that consume a channel's events: their stored policy, and the
//! one view of them downstream code reads.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::OnceLock};
use utoipa::ToSchema;

use super::{ChannelDefinition, ProviderBinding, Secret};
use crate::{
    ids::{ConsumerId, ConsumerName},
    profile::EventWorkerProfile,
};

/// An additional consumer of a channel, beside the implicit `default`. It
/// carries its own binding, profile and agent key, so it is stored whole;
/// a channel holds at most seven.
///
/// Every optional field is omitted when absent or at its default, so a
/// stored consumer keeps its bytes when read and written back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Consumer {
    pub id: ConsumerId,
    pub name: ConsumerName,
    /// 1..=64 printable characters for reply labels; absent means `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub profile: EventWorkerProfile,
    pub profile_digest: String,
    pub provider: ProviderBinding,
    /// The public hash of the consumer's agent key, `sha256:<64 hex>`.
    pub agent_key_hash: String,
    /// Whether a leading `@name` or `/to name` may address it.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub mentionable: bool,
    /// Whether this consumer answers non-human actors (custom events and
    /// hand-offs). Absent means its provider slug is configured as non-human.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_human: Option<bool>,
    #[serde(default, skip_serializing_if = "ConsumerReply::is_default")]
    pub reply: ConsumerReply,
    #[serde(default, skip_serializing_if = "PublishPolicy::is_disabled")]
    pub publish: PublishPolicy,
    /// Stored before its provider binding exists; never routed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    /// RFC 3339.
    pub created_at: String,
}

/// The presentation and policy of the `default` consumer. Its binding,
/// profile and agent key stay in the definition's top-level fields, so its
/// storage, conversation ids and tokens never change when routing is added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DefaultConsumer {
    pub name: ConsumerName,
    /// 1..=64 printable characters for reply labels; absent means `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether a leading `@name` or `/to name` may address it.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub mentionable: bool,
    /// Whether it answers non-human actors; absent means by provider slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_human: Option<bool>,
    #[serde(default, skip_serializing_if = "ConsumerReply::is_default")]
    pub reply: ConsumerReply,
    #[serde(default, skip_serializing_if = "PublishPolicy::is_disabled")]
    pub publish: PublishPolicy,
}

/// A consumer's own reply policy, overriding the channel's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsumerReply {
    /// Absent means the channel's `reply.auto_final`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_final: Option<bool>,
    /// Where automatic outputs go.
    #[serde(default, skip_serializing_if = "ReplyChoice::is_origin")]
    pub destination: ReplyChoice,
}

impl ConsumerReply {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Where a reply goes: back to the event's origin, or to the channel's
/// default reply destination.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplyChoice {
    #[default]
    Origin,
    Default,
}

impl ReplyChoice {
    pub fn is_origin(&self) -> bool {
        *self == Self::Origin
    }
}

/// Whether a consumer may publish hand-off events to other consumers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishPolicy {
    pub enabled: bool,
    /// The consumers it may target; empty means any routable consumer.
    #[serde(default)]
    pub targets: Vec<ConsumerId>,
}

impl PublishPolicy {
    /// Disabled with no targets: the default, which is omitted when stored.
    /// A disabled policy that still lists targets is kept, so it round-trips.
    pub fn is_disabled(&self) -> bool {
        !self.enabled && self.targets.is_empty()
    }
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

/// The one view of "which agent serves this turn" that turn, tool, context,
/// card and media code may use. Downstream code never reads the top-level
/// provider fields directly, so adding consumers needs no caller change.
#[derive(Debug, Clone, Copy)]
pub struct ConsumerView<'a> {
    /// Borrowed from a static for `default`, or from the routing policy.
    pub id: &'a ConsumerId,
    pub name: &'a str,
    /// The stored label, else the name.
    pub label: &'a str,
    pub profile: &'a EventWorkerProfile,
    pub profile_digest: &'a str,
    pub provider: &'a ProviderBinding,
    /// The public hash of the consumer's agent key, `sha256:<64 hex>`.
    pub agent_key_hash: &'a str,
    /// The consumer's agent key, used as its provider credential.
    pub credential: Option<&'a Secret>,
    pub mentionable: bool,
    /// The stored override; see [`ConsumerView::non_human_effective`].
    pub non_human: Option<bool>,
    pub reply: EffectiveReply,
    pub publish: &'a PublishPolicy,
    /// Stored before its binding exists; routing skips it.
    pub pending: bool,
}

/// A consumer's reply policy with the channel's defaults applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveReply {
    /// The consumer's `reply.auto_final`, else the channel's.
    pub auto_final: bool,
    pub destination: ReplyChoice,
}

impl ConsumerView<'_> {
    /// Effective non-human capability: the per-consumer override, else
    /// whether its provider slug is in the configured non-human set.
    pub fn non_human_effective(&self, non_human_providers: &BTreeSet<String>) -> bool {
        self.non_human
            .unwrap_or_else(|| non_human_providers.contains(self.provider.slug.as_str()))
    }
}

/// The `default` id, built once so every default view can borrow it.
fn default_id() -> &'static ConsumerId {
    static DEFAULT: OnceLock<ConsumerId> = OnceLock::new();
    DEFAULT.get_or_init(ConsumerId::default_consumer)
}

/// The publish policy of a consumer that stores none.
static PUBLISH_DISABLED: PublishPolicy = PublishPolicy {
    enabled: false,
    targets: Vec::new(),
};

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

    /// The default consumer: the top-level binding, named `default`, mentionable,
    /// replying to the origin under the channel's policy and never publishing.
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
            mentionable: true,
            non_human: None,
            reply: EffectiveReply {
                auto_final: self.reply.auto_final,
                destination: ReplyChoice::Origin,
            },
            publish: &PUBLISH_DISABLED,
            pending: false,
        }
    }
}
