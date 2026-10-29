//! Exact source attestation documents; these do not select a new event or Agent.
pub use crate::provider_bindings::{
    BindingMediaArtifact as MediaArtifactClaim, BindingMediaKind as MediaContinuationKind,
};
use serde::{Deserialize, Serialize};
/// Comparisons against the original sealed event capability.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventAttestationRequest {
    /// Original sealed event capability.
    pub event_ref: String,
    /// Original provider binding.
    pub provider_binding_id: String,
    /// Original channel.
    pub channel_id: String,
    /// Original transport conversation.
    pub conversation_id: String,
    /// Original event.
    pub event_id: String,
    /// Complete original Activity digest.
    pub activity_sha256: String,
    /// Original optional Gateway generation, never a Manager generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_generation: Option<u64>,
    /// Exact original sealed expiry, Unix seconds.
    pub event_expires_at: i64,
    /// Exact artifact proof; event-only attestation cannot authorize media.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<MediaArtifactClaim>,
}
/// Current authority result; false refuses the requested effect.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventAttestationResponse {
    /// Whether the complete source authority remains current.
    pub current: bool,
}
