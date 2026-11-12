//! Stable domain identity for a single durable Agent-log publication.

/// Allocate before the retry loop. The store validates and persists all three
/// fields; mutable aggregate revisions and lease holders are not identities.
/// Mongo adapters encode the digest using their existing generic Binary form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLogPublicationIdentity {
    pub key: String,
    pub fingerprint: [u8; 32],
    pub appended_at: String,
}
