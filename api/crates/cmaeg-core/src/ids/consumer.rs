//! Identifiers of the agents that consume a channel's events, their names
//! and the routing rules that select them.
use crate::ValidationError;
use serde::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

/// The identifier of the implicit consumer every channel has: the provider
/// binding, profile and agent key stored at the top level of its definition.
pub const DEFAULT_CONSUMER_ID: &str = "default";
const GENERATED_PREFIX: &str = "csm_";
const GENERATED_HEX_DIGITS: usize = 12;
/// Words a chat directive reads as an audience or as itself, never as one consumer.
const RESERVED_NAMES: [&str; 4] = ["all", "everyone", "here", "to"];
const MAX_NAME_BYTES: usize = 32;
const MAX_RULE_ID_BYTES: usize = 32;

/// A channel consumer's stable identifier: "default" or "csm_" + 12 lowercase hex.
/// Assigned by the gateway and never reused within a channel.
///
/// The grammar admits no `\x1f`, the separator of the gateway's identity
/// derivations, so an identifier can be joined into one unambiguously.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String, pattern = "^(default|csm_[0-9a-f]{12})$")]
pub struct ConsumerId(String);

impl ConsumerId {
    /// The implicit consumer backed by the definition's top-level fields.
    pub fn default_consumer() -> Self {
        Self(DEFAULT_CONSUMER_ID.to_owned())
    }

    /// Accept `default` or a gateway-assigned `csm_` identifier, exactly.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let generated = value.strip_prefix(GENERATED_PREFIX).is_some_and(|digits| {
            digits.len() == GENERATED_HEX_DIGITS
                && digits
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
        if value != DEFAULT_CONSUMER_ID && !generated {
            return Err(ValidationError::new(
                "/",
                "consumer id must be \"default\" or \"csm_\" followed by 12 lowercase hex digits",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn is_default(&self) -> bool {
        self.0 == DEFAULT_CONSUMER_ID
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ConsumerId {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ConsumerId> for String {
    fn from(id: ConsumerId) -> Self {
        id.0
    }
}

impl fmt::Display for ConsumerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A consumer's chat name, `^[a-z][a-z0-9_-]{1,31}$`, unique per channel.
/// Names are lowercase, so comparing two of them is exact.
///
/// Reserved: `all`, `everyone`, `here`, `to`, any `csm_` prefix, and
/// `default`, which only the default consumer may use. The grammar is a
/// property of the type; the reserved words are refused by the channel
/// readers, which know whether a name belongs to the default consumer and
/// can point at the offending field.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String, pattern = "^[a-z][a-z0-9_-]{1,31}$")]
pub struct ConsumerName(String);

impl ConsumerName {
    /// Accept exactly the grammar above.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let mut bytes = value.bytes();
        let valid = (2..=MAX_NAME_BYTES).contains(&value.len())
            && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
            && bytes.all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            });
        if !valid {
            return Err(ValidationError::new(
                "/",
                "consumer name must be a lowercase letter followed by 1 to 31 lowercase letters, digits, '_' or '-'",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// Whether no consumer may take this name: a chat audience word, a
    /// directive, or the shape of a generated consumer id.
    pub fn is_reserved(&self) -> bool {
        RESERVED_NAMES.contains(&self.0.as_str()) || self.0.starts_with(GENERATED_PREFIX)
    }

    /// Whether this is `default`, the name reserved for the default consumer.
    pub fn is_default(&self) -> bool {
        self.0 == DEFAULT_CONSUMER_ID
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ConsumerName {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ConsumerName> for String {
    fn from(name: ConsumerName) -> Self {
        name.0
    }
}

impl fmt::Display for ConsumerName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A routing rule's identifier, `^[a-z0-9_-]{1,32}$`, unique per channel.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String, pattern = "^[a-z0-9_-]{1,32}$")]
pub struct RuleId(String);

impl RuleId {
    /// Accept exactly the grammar above.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let valid = (1..=MAX_RULE_ID_BYTES).contains(&value.len())
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            });
        if !valid {
            return Err(ValidationError::new(
                "/",
                "rule id must contain 1 to 32 lowercase letters, digits, '_' or '-'",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RuleId {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<RuleId> for String {
    fn from(id: RuleId) -> Self {
        id.0
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
