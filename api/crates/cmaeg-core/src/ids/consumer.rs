//! Identifiers of the agents that consume a channel's events.
use crate::ValidationError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The identifier of the implicit consumer every channel has: the provider
/// binding, profile and agent key stored at the top level of its definition.
pub const DEFAULT_CONSUMER_ID: &str = "default";
const GENERATED_PREFIX: &str = "csm_";
const GENERATED_HEX_DIGITS: usize = 12;

/// A channel consumer's stable identifier: "default" or "csm_" + 12 lowercase hex.
/// Assigned by the gateway and never reused within a channel.
///
/// The grammar admits no `\x1f`, the separator of the gateway's identity
/// derivations, so an identifier can be joined into one unambiguously.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
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
