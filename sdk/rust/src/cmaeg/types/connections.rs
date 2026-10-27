//! Platform-neutral channel connection documents.
use serde::{Deserialize, Serialize};

/// Supported regional bot-resource connection inventory.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePlatform {
    /// International Lark bot API.
    Lark,
    /// Mainland Feishu bot API.
    Feishu,
}

/// Provenance of a current-subject-visible bot connection.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceConnectionSource {
    /// The authenticated subject's connection.
    Personal,
    /// An allowed organization connection visible to the subject.
    Organization,
}

/// Existing connection metadata, without credentials or implied channel-key grants.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceConnection {
    /// Exact visible connection identity.
    pub user_service_id: String,
    /// Exact linked catalog service identity.
    pub catalog_service_id: String,
    /// Human-readable connection label.
    pub name: String,
    /// Validated regional service.
    pub platform: ResourcePlatform,
    /// Metadata provenance, without implied channel-key scope.
    pub source: ResourceConnectionSource,
}

/// Bounded current-subject inventory, including eligible shared organization connections.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceConnectionList {
    /// Existing eligible connections; choosing one does not change any grant.
    pub items: Vec<ResourceConnection>,
}

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

/// Server tool metadata; does not grant channel, source or event permission.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentToolCapability {
    /// Exact MCP tool name.
    pub name: String,
    /// Exact MCP tool description.
    pub description: String,
    /// MCP read-only behavior hint.
    pub read_only: bool,
    /// MCP destructive behavior hint.
    pub destructive: bool,
    /// MCP idempotency behavior hint.
    pub idempotent: bool,
    /// MCP open-world behavior hint.
    pub open_world: bool,
}

/// Gateway connection capabilities.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityView {
    /// Deployment tool catalog; absent or null means availability was not reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_tools: Option<Vec<AgentToolCapability>>,
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
    /// Separate native app authoring gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_lark_writes_enabled: Option<bool>,
    /// Native webhook intake gate, independent of authoring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_lark_intake_enabled: Option<bool>,
    /// Native Lark card and button issuance is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_lark_controls_enabled: Option<bool>,
    /// Native Lark form issuance is available and requires native controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_lark_forms_enabled: Option<bool>,
    /// Native Telegram owners may enable durable edit and reaction notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_telegram_message_events_enabled: Option<bool>,
    /// Opt-in native Discord thread history is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_discord_thread_history_enabled: Option<bool>,
    /// Whether native Discord incoming reaction events are enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_discord_reactions_enabled: Option<bool>,
    /// New direct Discord source and token writes are available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_discord_writes_enabled: Option<bool>,
    /// New native channels can be discovered without operator record selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_discord_auto_activation_enabled: Option<bool>,
    /// Per-process native channel selection bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_discord_capacity: Option<usize>,
    /// New native Meta WhatsApp source and credential writes are available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_whatsapp_writes_enabled: Option<bool>,
    /// Owner opt-in to existing OBA groups, independent of group issuance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_whatsapp_groups_writes_enabled: Option<bool>,
    /// Durable admission/recovery and routing authoring prerequisites are enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_whatsapp_auto_activation_enabled: Option<bool>,
    /// Enables new dynamic-source and partition writes.
    pub dynamic_sources_writes_enabled: bool,
    /// Enables Discord/WhatsApp raw-event verification writes independently.
    pub relay_raw_verification_writes_enabled: bool,
    /// Enables explicit bindings for embedded Lark/Feishu media reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lark_resource_writes_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Whether owners may enroll new Lark/Feishu message-effect bindings.
    pub lark_message_effects_writes_enabled: Option<bool>,
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
