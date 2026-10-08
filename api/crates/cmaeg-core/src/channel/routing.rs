//! A channel's multi-consumer routing policy: which consumers exist beside
//! `default`, and how an admitted event selects the ones it reaches.
use serde::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

use super::{Consumer, DefaultConsumer, SourceId};
use crate::{
    ids::{ConsumerId, RuleId},
    ValidationError,
};

/// The routing policy schema this image reads.
pub const ROUTING_POLICY_VERSION: u16 = 1;
/// Consumers beside `default`; eight in all.
pub const MAX_ADDITIONAL_CONSUMERS: usize = 7;
pub const MAX_RULES: usize = 32;
/// The most entries in one `RuleMatch` list.
pub const MAX_MATCH_ENTRIES: usize = 64;
/// The longest `RuleMatch` entry, and the longest pattern.
pub const MAX_MATCH_ENTRY_BYTES: usize = 512;
/// The largest `max_targets` a policy may declare.
pub const MAX_TARGETS: u8 = 8;
const DEFAULT_MAX_TARGETS: u8 = 4;

/// Multi-consumer routing. Each admitted event resolves to one target set:
/// explicit addressing when the channel allows it, else the first matching
/// rule, else `default_targets`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingPolicy {
    /// Always 1.
    pub version: u16,
    pub default_consumer: DefaultConsumer,
    /// Additional consumers, 0..=7.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumers: Vec<Consumer>,
    /// 0..=32 rules; the first match wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RoutingRule>,
    /// The targets of an event no directive or rule selects: 1..=max_targets.
    pub default_targets: Vec<ConsumerId>,
    #[serde(default)]
    pub addressing: Addressing,
    /// The most consumers one event may target, 1..=8.
    #[serde(default = "default_max_targets")]
    pub max_targets: u8,
    #[serde(default)]
    pub reply_label: ReplyLabel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reply: Option<DefaultReply>,
    #[serde(default, skip_serializing_if = "AgentEventPolicy::is_default")]
    pub agent_events: AgentEventPolicy,
}

fn default_max_targets() -> u8 {
    DEFAULT_MAX_TARGETS
}

/// A rule: events matching `when` target `targets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingRule {
    pub id: RuleId,
    #[serde(default)]
    pub when: RuleMatch,
    /// 1..=max_targets consumers.
    pub targets: Vec<ConsumerId>,
}

/// What a rule matches: AND across non-empty fields, OR within a list. An
/// empty match admits every event. Each list holds at most 64 entries of at
/// most 512 bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleMatch {
    /// Source slots. A rule naming only sources that are not attached is
    /// valid and inert.
    #[serde(default)]
    pub sources: Vec<SourceId>,
    #[serde(default)]
    pub kinds: Vec<ActivityKindName>,
    /// `activity.conversation.id`, exact.
    #[serde(default)]
    pub chats: Vec<String>,
    /// `activity.actor.id`, exact.
    #[serde(default)]
    pub senders: Vec<String>,
    /// CloudEvent `type`: exact, or a prefix ending in one `*`.
    #[serde(default)]
    pub event_types: Vec<Pattern>,
    /// CloudEvent `source`: exact, or a prefix ending in one `*`.
    #[serde(default)]
    pub event_sources: Vec<Pattern>,
}

/// An activity's kind, by name.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKindName {
    Message,
    Command,
    Interaction,
    Event,
}

/// A match pattern of 1..=512 bytes: exact, or a prefix when it ends in
/// `*`. At most one `*`, and only as the final character.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String, min_length = 1, max_length = 512, pattern = "^[^*]*\\*?$")]
pub struct Pattern(String);

impl Pattern {
    /// Accept exactly the grammar above.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let star = value.find('*');
        if value.is_empty()
            || value.len() > MAX_MATCH_ENTRY_BYTES
            || star.is_some_and(|index| index + 1 != value.len())
        {
            return Err(ValidationError::new(
                "/",
                "pattern must contain 1 to 512 bytes with at most one '*', as its final character",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Pattern {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<Pattern> for String {
    fn from(pattern: Pattern) -> Self {
        pattern.0
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Gateway routing directives; each can be switched off per channel. None of
/// them is a reserved product command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Addressing {
    /// A leading `@name`.
    #[serde(default = "yes")]
    pub mentions: bool,
    /// `/to name[,name]`.
    #[serde(default = "yes")]
    pub command: bool,
    /// The `cmaegtargets` CloudEvent extension.
    #[serde(default = "yes")]
    pub event_field: bool,
    /// Answer an unknown target with a notice.
    #[serde(default = "yes")]
    pub unknown_target_notice: bool,
}

impl Default for Addressing {
    fn default() -> Self {
        Self {
            mentions: true,
            command: true,
            event_field: true,
            unknown_target_notice: true,
        }
    }
}

fn yes() -> bool {
    true
}

/// When chat replies carry the `[<label>] ` prefix.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplyLabel {
    #[default]
    Auto,
    Always,
    Never,
}

/// The channel's default reply destination. Never sealed: replies resolve it
/// from the current definition when a message is sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum DefaultReply {
    /// A chat of the native Telegram source.
    TelegramChat {
        source_id: SourceId,
        chat_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    /// A route of a NyxID relay source.
    RelayConversation {
        source_id: SourceId,
        route_id: String,
    },
    /// The channel's result webhook sinks.
    Sinks {},
}

/// Limits on events agents publish to other consumers of the channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentEventPolicy {
    /// 1..=4.
    #[serde(default = "two")]
    pub max_hops: u8,
    /// 1..=30, at most `per_channel_per_minute`.
    #[serde(default = "ten")]
    pub per_consumer_per_minute: u16,
    /// 1..=60.
    #[serde(default = "thirty")]
    pub per_channel_per_minute: u16,
}

impl Default for AgentEventPolicy {
    fn default() -> Self {
        Self {
            max_hops: two(),
            per_consumer_per_minute: ten(),
            per_channel_per_minute: thirty(),
        }
    }
}

impl AgentEventPolicy {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn two() -> u8 {
    2
}

fn ten() -> u16 {
    10
}

fn thirty() -> u16 {
    30
}
