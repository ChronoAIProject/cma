//! Stable, derived names for a channel's source slots.
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Platform, Source};
use crate::{activity::SourceKind, ValidationError};

/// The id reserved for hand-offs between a channel's agents. No source can
/// be configured under it.
pub const AGENT_SOURCE_ID: &str = "agent";

const RELAY_PREFIX: &str = "relay_";
const PLATFORMS: [Platform; 6] = [
    Platform::Telegram,
    Platform::Lark,
    Platform::Feishu,
    Platform::Discord,
    Platform::Slack,
    Platform::Whatsapp,
];

/// A source slot's stable id, derived from a configured source and never stored
/// with it, though routing policies refer to slots by it: "telegram", "relay_<platform>"
/// ("relay_telegram", "relay_lark", "relay_feishu", "relay_discord", "relay_slack",
/// "relay_whatsapp"), "custom", or the reserved "agent" (hand-offs only; never configurable).
///
/// An id names a slot rather than a source instance. A channel holds at most
/// one source per variant and relay platform, so the id is unique within it,
/// and a relay that is detached and later re-attached gets the same id back.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String)]
pub struct SourceId(String);

impl SourceId {
    /// Accept exactly the grammar above; spellings such as NyxID's
    /// `telegram-new` or a capitalized variant are not ids.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let known = matches!(value, "telegram" | "custom" | AGENT_SOURCE_ID)
            || value.strip_prefix(RELAY_PREFIX).is_some_and(|platform| {
                PLATFORMS
                    .iter()
                    .any(|candidate| candidate.as_str() == platform)
            });
        if known {
            Ok(Self(value.to_owned()))
        } else {
            Err(ValidationError::new(
                "/source_id",
                "must be telegram, relay_<platform>, custom or agent",
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the reserved hand-off id rather than a configurable source.
    pub fn is_agent(&self) -> bool {
        self.0 == AGENT_SOURCE_ID
    }

    /// The reserved hand-off id, recorded on events agents publish.
    pub fn agent() -> Self {
        Self(AGENT_SOURCE_ID.to_owned())
    }

    fn telegram() -> Self {
        Self("telegram".to_owned())
    }

    fn relay(platform: Platform) -> Self {
        Self(format!("{RELAY_PREFIX}{}", platform.as_str()))
    }

    fn custom() -> Self {
        Self("custom".to_owned())
    }
}

impl TryFrom<String> for SourceId {
    type Error = ValidationError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<SourceId> for String {
    fn from(value: SourceId) -> Self {
        value.0
    }
}

impl Source {
    /// The slot this configured source occupies.
    pub fn source_id(&self) -> SourceId {
        match self {
            Self::Telegram { .. } => SourceId::telegram(),
            Self::NyxidRelay { platform, .. } => SourceId::relay(*platform),
            Self::Custom { .. } => SourceId::custom(),
        }
    }
}

impl SourceKind {
    /// The slot of the source an activity arrived on.
    pub fn source_id(&self) -> SourceId {
        match self {
            Self::Telegram { .. } => SourceId::telegram(),
            Self::NyxidRelay { platform, .. } => SourceId::relay(*platform),
            Self::Custom => SourceId::custom(),
        }
    }
}
