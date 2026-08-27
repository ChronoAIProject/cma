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

    /// Whether this identifier belongs to CMA's deterministic Trigger lane.
    pub fn is_trigger_managed(&self) -> bool {
        self.0.starts_with("sesn_trg_")
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

/// Identifier of a reusable environment bundle.
///
/// Minted here rather than by the store so a caller never has to write a
/// record before it can be referred to, and so two stores cannot disagree on
/// what an identifier looks like.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnvironmentProfileId(pub String);

impl EnvironmentProfileId {
    /// Mint a fresh identifier.
    pub fn generate() -> Self {
        Self(format!("envp_{}", uuid::Uuid::new_v4().simple()))
    }

    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this identifier belongs to CMA's deterministic Trigger lane.
    pub fn is_trigger_managed(&self) -> bool {
        self.0.starts_with("envp_trg_")
    }
}

impl std::fmt::Display for EnvironmentProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifier of one sealed price card.
///
/// Minted before storage for the same reason as an environment profile id: a
/// mis-routed identifier is obvious in a log rather than becoming a lookup in
/// the wrong collection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RateCardId(pub String);

impl RateCardId {
    /// Mint a fresh identifier.
    pub fn generate() -> Self {
        Self(format!("rate_{}", uuid::Uuid::new_v4().simple()))
    }

    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RateCardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Selects which runtime implementation backs a session.
///
/// Deliberately opaque: the core stores and forwards this value but never
/// interprets it. Interpretation belongs to the process that wires concrete
/// runtime implementations together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KernelId(pub String);

impl KernelId {
    /// Whether this names nothing.
    ///
    /// The default is empty rather than a kernel, because which kernels exist
    /// is not this layer's to know. Something that does know fills it in.
    pub fn is_unset(&self) -> bool {
        self.0.trim().is_empty()
    }

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

    #[test]
    fn generated_profile_ids_are_unique_and_prefixed() {
        // The prefix is what makes a mis-routed identifier obvious in a log
        // rather than a silent lookup against the wrong collection.
        let a = EnvironmentProfileId::generate();
        let b = EnvironmentProfileId::generate();
        assert_ne!(a, b, "two generated ids must differ");
        assert!(a.as_str().starts_with("envp_"), "got {a}");
    }

    #[test]
    fn generated_rate_card_ids_are_unique_and_prefixed() {
        let a = RateCardId::generate();
        let b = RateCardId::generate();
        assert_ne!(a, b, "two generated ids must differ");
        assert!(a.as_str().starts_with("rate_"), "got {a}");
    }
}
