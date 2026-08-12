//! Opaque identifiers used across the core.
//!
//! Every id is a newtype over a string so that callers cannot accidentally
//! substitute one kind of id for another.

use serde::{Deserialize, Serialize};

/// Identifier of a session. Stable across an idle to revive cycle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    /// Mint a fresh identifier.
    pub fn generate() -> Self {
        Self(format!("sesn_{}", uuid::Uuid::new_v4().simple()))
    }

    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The authenticated principal that owns a session. The core never models a
/// user directory of its own; this is whatever the identity provider asserts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubjectId(pub String);

impl SubjectId {
    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SubjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Selects which runtime implementation backs a session.
///
/// Deliberately opaque: the core stores and forwards this value but never
/// interprets it. Interpretation belongs to the process that wires concrete
/// runtime implementations together.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KernelId(pub String);

impl KernelId {
    /// Wrap a selector string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for KernelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_session_ids_are_unique_and_prefixed() {
        let a = SessionId::generate();
        let b = SessionId::generate();
        assert_ne!(a, b, "two generated ids must differ");
        assert!(a.as_str().starts_with("sesn_"), "got {a}");
    }

    #[test]
    fn selector_round_trips_through_its_accessor() {
        let id = KernelId::new("example-runtime");
        assert_eq!(id.as_str(), "example-runtime");
        assert_eq!(id.to_string(), "example-runtime");
    }
}
