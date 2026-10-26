//! Platform-neutral channel connection documents.
use serde::{Deserialize, Serialize};

/// Durable state of a channel connection.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Awaiting a platform choice.
    Draft,
    /// Waiting for platform registration.
    AwaitingPlatform,
    /// Ready for channel attachment.
    Ready,
    /// Cancelled by its owner.
    Cancelled,
    /// Setup capability expired.
    Expired,
}

/// Platform choices supported by this gateway.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PlatformCapability {
    /// Stable platform identifier.
    pub id: String,
    /// Human-readable platform name.
    pub label: String,
    /// Available connection methods.
    pub connection_methods: Vec<String>,
    /// Capabilities the owner may select during setup.
    pub optional_capabilities: Vec<String>,
}

/// Gateway connection capabilities.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityView {
    /// Additive source-policy features; absent on older gateways.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_features: Vec<String>,
    /// Supported platforms.
    pub platforms: Vec<PlatformCapability>,
}

/// Owner-visible connection state without platform credentials.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionView {
    /// Opaque connection identifier.
    pub connection_ref: String,
    /// Durable lifecycle state.
    pub status: Status,
    /// Platform-provided label, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Public platform URL, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Bounded owner setup capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_url: Option<String>,
}

/// One connection page; empty pages may still carry a continuation.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionPage {
    /// Connections visible on this page.
    pub items: Vec<ConnectionView>,
    /// Opaque cursor for the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after: Option<String>,
}

/// Reserve a connection using an admitted originating event.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateConnection {
    /// Caller-reserved UUID for durable recovery.
    pub record_id: String,
    /// Original admitted event reference.
    pub event_ref: String,
    /// Optional user-facing suggestion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_name: Option<String>,
}
