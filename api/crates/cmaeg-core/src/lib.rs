//! Pure protocol and identity types shared by the CMA Event Gateway.

use std::fmt;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Channel definitions and their validation rules.
pub mod channel;

/// Neutral inbound activity produced by an event source.
pub mod activity;
pub mod request_audit;

/// Paginated reference-only native conversation history.
pub mod conversation_history;
/// Exact file and derived-text claims for parent-bound media continuations.
pub mod media_continuation;
/// Verified opt-in native thread contract.
pub mod thread;
/// Bounded native thread lifecycle wire contract.
pub mod thread_lifecycle;

/// Neutral messages emitted by an agent turn.
pub mod message;
pub mod message_effect;

/// Chat text conventions, such as bot addresses, shared by routing and rendering.
pub mod chat;

/// The pure routing decision: which consumers an admitted event reaches.
pub mod routing;

/// CloudEvents 1.0 envelope types.
pub mod cloudevent;

/// RFC 9457 problem documents.
pub mod problem;

/// Provider-facing agent profile snapshots.
pub mod profile;

/// Sealed channel and event-reference primitives.
pub mod seal;

/// Deterministic identifiers used across CMAEG boundaries.
pub mod ids;

/// OpenAPI views of protocol serialization shapes.
pub mod schema;

/// A first validation failure, expressed as a JSON pointer into the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidationError {
    /// Pointer to the invalid value, such as `/profile/model`.
    pub pointer: String,
    /// Stable human-readable reason for the refusal.
    pub message: String,
}

impl ValidationError {
    /// Construct a validation failure at a JSON pointer.
    pub fn new(pointer: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            pointer: pointer.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.pointer, self.message)
    }
}

impl std::error::Error for ValidationError {}

/// Return a validation failure for a string bound.
pub(crate) fn require_string(
    value: &str,
    pointer: &str,
    min: usize,
    max: usize,
) -> Result<(), ValidationError> {
    if value.len() < min {
        return Err(ValidationError::new(
            pointer,
            format!("must contain at least {min} bytes"),
        ));
    }
    if value.len() > max {
        return Err(ValidationError::new(
            pointer,
            format!("must contain at most {max} bytes"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn exposes_the_protocol_module_tree() {
        assert_eq!(
            [
                "channel",
                "activity",
                "message",
                "cloudevent",
                "problem",
                "profile",
                "seal",
                "ids",
            ]
            .len(),
            8
        );
    }
}
