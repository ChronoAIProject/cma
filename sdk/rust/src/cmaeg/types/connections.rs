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
    /// Native connection workflows; not the relay ingestion platform list.
    pub platforms: Vec<PlatformCapability>,
    /// Relay authoring metadata, absent on older servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_authoring: Option<RelayAuthoringCapabilities>,
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

/// Deployment gates for new relay authoring; existing exact records remain readable.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAuthoringCapabilities {
    /// Opt-in native Discord thread history is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_discord_thread_history_enabled: Option<bool>,
    /// New direct Discord source and token writes are available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_discord_writes_enabled: Option<bool>,
    /// New native channels can be discovered without operator record selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_discord_auto_activation_enabled: Option<bool>,
    /// Per-process native channel selection bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_discord_capacity: Option<usize>,
    /// Enables new dynamic-source and partition writes.
    pub dynamic_sources_writes_enabled: bool,
    /// Enables Discord/WhatsApp raw-event verification writes independently.
    pub relay_raw_verification_writes_enabled: bool,
    /// Enables explicit bindings for embedded Lark/Feishu media reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lark_resource_writes_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Whether owner-initiated message preparation and commit are enabled.
    pub outbound_messages_writes_enabled: Option<bool>,
    /// Whether the configured relay accepts native attachments; platform limits still apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_media_writes_enabled: Option<bool>,
    /// Enables new additional-consumer routing writes.
    pub multi_consumer_writes_enabled: bool,
    /// Enables new native-bot mention policy writes.
    pub native_bot_mentions_writes_enabled: bool,
    /// Enables new verified thread-following writes.
    pub thread_contract_writes_enabled: bool,
    /// Supported relay ingestion platforms, independently of native connection setup.
    pub platforms: Vec<RelayPlatformCapability>,
}
/// Relay verification choices for one messaging platform.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayPlatformCapability {
    /// Stable platform identifier.
    pub id: String,
    /// Display name.
    pub label: String,
    /// Supported verification choices, not a delivery-health attestation.
    pub verification_modes: Vec<RelayVerificationCapability>,
}
/// A supported relay verification mode and its authoring availability.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayVerificationCapability {
    /// native_event, raw_event or trusted_normalized; future identifiers are retained.
    pub id: String,
    /// Whether new writes selecting this mode are accepted.
    pub writes_enabled: bool,
}
