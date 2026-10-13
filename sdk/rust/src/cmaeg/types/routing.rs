//! Multi-consumer routing wire types for the independent CMAEG v1 protocol.
//! Channels expose them read-only in `PublicChannelDefinition.routing`.
use super::{EventWorkerProfile, ProviderBinding};
use serde::{Deserialize, Serialize};

/// A channel consumer's id: `default` or `csm_` and 12 lowercase hex digits.
pub type ConsumerId = String;

/// A consumer's chat name, `^[a-z][a-z0-9_-]{1,31}$`, unique per channel.
pub type ConsumerName = String;

/// A routing rule's id, `^[a-z0-9_-]{1,32}$`, unique per channel.
pub type RuleId = String;

/// A source slot: `telegram`, `relay_<platform>`, `custom` or `agent`.
pub type SourceId = String;

/// An exact match, or a prefix match when it ends in one `*`.
pub type Pattern = String;

/// CMAEG `ConsumerSummary` wire shape: which of a routed channel's consumers
/// a context response was read for.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsumerSummary {
    /// Wire field `id`.
    pub id: ConsumerId,
    /// Wire field `name`.
    pub name: ConsumerName,
    /// The consumer's reply label, else its name.
    pub label: String,
}

/// CMAEG `ActivityKindName` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityKindName {
    /// Wire value `message`.
    #[serde(rename = "message")]
    Message,
    /// Wire value `command`.
    #[serde(rename = "command")]
    Command,
    /// Wire value `interaction`.
    #[serde(rename = "interaction")]
    Interaction,
    /// Wire value `event`.
    #[serde(rename = "event")]
    Event,
}

/// CMAEG `ReplyChoice` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplyChoice {
    /// Reply to the event's origin.
    #[serde(rename = "origin")]
    Origin,
    /// Reply to the channel's default reply destination.
    #[serde(rename = "default")]
    Default,
}

/// CMAEG `ReplyLabel` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplyLabel {
    /// Wire value `auto`.
    #[serde(rename = "auto")]
    Auto,
    /// Wire value `always`.
    #[serde(rename = "always")]
    Always,
    /// Wire value `never`.
    #[serde(rename = "never")]
    Never,
}

/// CMAEG `ConsumerReply` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsumerReply {
    /// Absent means the channel's `reply.auto_final`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_final: Option<bool>,
    /// Where automatic outputs go; absent means `origin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<ReplyChoice>,
}

/// CMAEG `PublishPolicy` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishPolicy {
    /// Whether the consumer may publish hand-off events.
    pub enabled: bool,
    /// Consumers it may target; empty or absent means any routable consumer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<ConsumerId>>,
}

/// CMAEG `Consumer` wire shape: an additional consumer beside `default`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Consumer {
    /// Wire field `id`.
    pub id: ConsumerId,
    /// Wire field `name`.
    pub name: ConsumerName,
    /// Reply label; absent means `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Wire field `profile`.
    pub profile: EventWorkerProfile,
    /// Wire field `profile_digest`.
    pub profile_digest: String,
    /// Wire field `provider`.
    pub provider: ProviderBinding,
    /// The public hash of the consumer's agent key; never the key.
    pub agent_key_hash: String,
    /// Whether `@name` or `/to name` may address it; absent means true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentionable: Option<bool>,
    /// Whether it answers non-human actors; absent means by provider slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_human: Option<bool>,
    /// Wire field `reply`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<ConsumerReply>,
    /// Wire field `publish`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<PublishPolicy>,
    /// Stored before its provider binding exists; never routed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<bool>,
    /// Wire field `created_at`.
    pub created_at: String,
}

/// CMAEG `DefaultConsumer` wire shape: how `default` is presented.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct DefaultConsumer {
    /// Wire field `name`.
    pub name: ConsumerName,
    /// Reply label; absent means `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether `@name` or `/to name` may address it; absent means true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentionable: Option<bool>,
    /// Whether it answers non-human actors; absent means by provider slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_human: Option<bool>,
    /// Wire field `reply`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<ConsumerReply>,
    /// Wire field `publish`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<PublishPolicy>,
}

/// CMAEG `RuleMatch` wire shape: AND across lists, OR within one.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleMatch {
    /// Wire field `sources`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<SourceId>>,
    /// Wire field `kinds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<ActivityKindName>>,
    /// Wire field `chats`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chats: Option<Vec<String>>,
    /// Wire field `senders`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub senders: Option<Vec<String>>,
    /// Wire field `event_types`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_types: Option<Vec<Pattern>>,
    /// Wire field `event_sources`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_sources: Option<Vec<Pattern>>,
}

/// CMAEG `RoutingRule` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingRule {
    /// Wire field `id`.
    pub id: RuleId,
    /// Wire field `when`; absent matches every event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<RuleMatch>,
    /// Wire field `targets`.
    pub targets: Vec<ConsumerId>,
}

/// CMAEG `Addressing` wire shape; an absent switch is on.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Addressing {
    /// A leading `@name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentions: Option<bool>,
    /// `/to name[,name]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<bool>,
    /// The `cmaegtargets` CloudEvent extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_field: Option<bool>,
    /// Answer an unknown target with a notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_target_notice: Option<bool>,
}

/// CMAEG `DefaultReply` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DefaultReply {
    /// A chat of the native Telegram source.
    #[serde(rename = "telegram_chat")]
    TelegramChat {
        /// Wire field `source_id`.
        source_id: SourceId,
        /// Wire field `chat_id`.
        chat_id: String,
        /// Wire field `thread_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    /// A route of a NyxID relay source.
    #[serde(rename = "relay_conversation")]
    RelayConversation {
        /// Wire field `source_id`.
        source_id: SourceId,
        /// Wire field `route_id`.
        route_id: String,
    },
    /// The channel's result webhook sinks.
    #[serde(rename = "sinks")]
    Sinks {},
}

/// CMAEG `AgentEventPolicy` wire shape; absent limits take their defaults.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEventPolicy {
    /// Wire field `max_hops`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_hops: Option<u64>,
    /// Wire field `per_consumer_per_minute`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_consumer_per_minute: Option<u64>,
    /// Wire field `per_channel_per_minute`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_channel_per_minute: Option<u64>,
}

/// CMAEG `RoutingPolicy` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingPolicy {
    /// Wire field `version`.
    pub version: u64,
    /// Wire field `default_consumer`.
    pub default_consumer: DefaultConsumer,
    /// Additional consumers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumers: Option<Vec<Consumer>>,
    /// First-match rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<RoutingRule>>,
    /// Wire field `default_targets`.
    pub default_targets: Vec<ConsumerId>,
    /// Wire field `addressing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressing: Option<Addressing>,
    /// Wire field `max_targets`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_targets: Option<u64>,
    /// Wire field `reply_label`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_label: Option<ReplyLabel>,
    /// Wire field `default_reply`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reply: Option<DefaultReply>,
    /// Wire field `agent_events`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_events: Option<AgentEventPolicy>,
}
