//! Environment configuration for the CMA Event Gateway.

use std::{env, net::SocketAddr};

use crate::providers::ProviderRegistry;
use cmaeg_core::channel::Secret;
use reqwest::Url;
mod origins;
pub(crate) use origins::validate_origin;
use origins::{parse_api_advertisement, required_origin, validate_public_api_url};

pub mod activity_history;
mod cache;
mod console;
mod delivery;
mod duration;
use duration::positive_duration;
mod graduated;
mod ingress;
mod management;
pub use management::ManagementConfig;
mod media;
mod native_discord;
pub mod owner_keys;
mod routing;
pub(crate) mod secrets;
mod service;
mod service_account;
pub use service::*;
mod stt;
pub mod transformer;
mod turns;
pub use cache::*;
pub use console::*;
pub use delivery::*;
pub use ingress::*;
pub(crate) use management::{
    channel_cancel_create_enabled, channel_create_fence_required, channel_mutation_custody_enabled,
    cma_record_origin_aliases,
};
pub use media::{
    MediaConfig, MediaToolProviders, CMAEG_MEDIA_CACHE_BYTES, CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR,
    CMAEG_MEDIA_ENABLED, CMAEG_MEDIA_INFLIGHT_BYTES, CMAEG_MEDIA_LINK_TTL_SECONDS,
    CMAEG_MEDIA_MAX_BYTES, CMAEG_MEDIA_MAX_JOBS, CMAEG_MEDIA_PREPARE_BUDGET_MS,
    CMAEG_MEDIA_RATE_PER_MINUTE, CMAEG_MEDIA_RESPONSE_BYTES, CMAEG_MEDIA_TOOL_PROVIDERS,
};
pub use native_discord::NativeDiscordConfig;
pub(crate) use routing::github_writes_enabled;
pub use routing::{RoutingConfig, CMAEG_NON_HUMAN_PROVIDERS};
pub use service_account::*;
pub use stt::*;
pub use transformer::{TransformerConfig, CMAEG_MEDIA_FFMPEG_DIR, CMAEG_MEDIA_VIDEO_FRAMES};
pub use turns::*;

/// The listener address used by CMAEG.
pub const CMAEG_LISTEN: &str = "CMAEG_LISTEN";
pub const CMAEG_METRICS_LISTEN: &str = "CMAEG_METRICS_LISTEN";
pub const CMAEG_PROVIDERS: &str = "CMAEG_PROVIDERS";
pub const CMAEG_PROVIDER_CONNECT_TIMEOUT_MS: &str = "CMAEG_PROVIDER_CONNECT_TIMEOUT_MS";
pub const CMAEG_PROVIDER_REQUEST_TIMEOUT_MS: &str = "CMAEG_PROVIDER_REQUEST_TIMEOUT_MS";
pub const CMAEG_PROVIDER_MAX_INFLIGHT: &str = "CMAEG_PROVIDER_MAX_INFLIGHT";
pub const CMAEG_PUBLIC_URL: &str = "CMAEG_PUBLIC_URL";
pub const CMAEG_CONSOLE_PUBLIC_URL: &str = "CMAEG_CONSOLE_PUBLIC_URL";
pub const CMAEG_PUBLIC_API_URL: &str = "CMAEG_PUBLIC_API_URL";
pub const CMAEG_PUBLIC_API_ALIASES: &str = "CMAEG_PUBLIC_API_ALIASES";
pub const CMAEG_ADVERTISE_PUBLIC_API_URL: &str = "CMAEG_ADVERTISE_PUBLIC_API_URL";
pub const CMAEG_MASTER_KEYS: &str = "CMAEG_MASTER_KEYS";
pub const CMAEG_MASTER_KEYS_FILE: &str = "CMAEG_MASTER_KEYS_FILE";
pub const CMAEG_CMA_BASE_URL: &str = "CMAEG_CMA_BASE_URL";
pub const CMAEG_CMA_RECORD_ORIGIN_ALIASES: &str = "CMAEG_CMA_RECORD_ORIGIN_ALIASES";
pub const CMAEG_CMA_SERVICE_TOKEN: &str = "CMAEG_CMA_SERVICE_TOKEN";
pub const CMAEG_CMA_SERVICE_TOKEN_FILE: &str = "CMAEG_CMA_SERVICE_TOKEN_FILE";
pub const CMAEG_NYXID_BASE_URL: &str = "CMAEG_NYXID_BASE_URL";
pub(crate) const CMAEG_NYXID_TURN_TRACE_ENABLED: &str = "CMAEG_NYXID_TURN_TRACE_ENABLED";
pub const CMAEG_TOOLS_NYXID_SLUG: &str = "CMAEG_TOOLS_NYXID_SLUG";

/// Configuration errors that prevent the service from binding safely.
#[derive(Debug)]
pub enum ConfigError {
    /// The configured listener was not a valid socket address.
    InvalidListen {
        name: &'static str,
        value: String,
        source: std::net::AddrParseError,
    },
    /// The provider registry JSON did not satisfy the outbound-call policy.
    InvalidProviders { detail: String },
    /// A duration setting was not a positive bounded integer.
    InvalidDuration { name: &'static str, value: String },
    /// The global provider concurrency cap was invalid.
    InvalidMaxInflight { value: String },
    /// A management-plane origin, secret, or slug was invalid.
    InvalidManagement { detail: String },
    /// An inbound media setting was invalid.
    InvalidMedia { detail: String },
    /// The configured media transformer failed its startup checks.
    MediaTransformer { detail: String },
    /// A console setting was invalid.
    InvalidConsole { detail: String },
    /// A multi-consumer routing setting was invalid.
    Routing(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidListen {
                name,
                value,
                source,
            } => {
                write!(formatter, "{name}={value:?} is invalid: {source}")
            }
            Self::InvalidProviders { detail } => {
                write!(formatter, "{CMAEG_PROVIDERS} is invalid: {detail}")
            }
            Self::InvalidDuration { name, value } => {
                write!(
                    formatter,
                    "{name}={value:?} is invalid: expected a positive millisecond value"
                )
            }
            Self::InvalidMaxInflight { value } => {
                write!(
                    formatter,
                    "{CMAEG_PROVIDER_MAX_INFLIGHT}={value:?} is invalid: expected a positive integer"
                )
            }
            Self::InvalidManagement { detail } => {
                write!(formatter, "management configuration is invalid: {detail}")
            }
            Self::InvalidMedia { detail } => {
                write!(formatter, "media configuration is invalid: {detail}")
            }
            Self::MediaTransformer { detail } => {
                write!(formatter, "media transformer unavailable: {detail}")
            }
            Self::InvalidConsole { detail } => {
                write!(formatter, "console configuration is invalid: {detail}")
            }
            Self::Routing(detail) => {
                write!(formatter, "routing configuration is invalid: {detail}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Process configuration loaded once at startup.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address on which the HTTP service accepts requests.
    pub listen: SocketAddr,
    pub metrics_listen: SocketAddr,
    /// Providers explicitly allowed to receive outbound CMAEG requests.
    pub providers: ProviderRegistry,
    /// TCP/TLS connection establishment timeout for provider clients.
    pub provider_connect_timeout: std::time::Duration,
    /// Provider request timeout for non-streaming setup calls.
    pub provider_request_timeout: std::time::Duration,
    /// Deployment-wide upper bound applied to each provider semaphore.
    pub provider_max_inflight: usize,
    /// Configuration required to expose the channel management API.
    pub management: ManagementConfig,
    pub cma_service_account: Option<ServiceAccountConfig>,
    pub channel_cache: ChannelCacheConfig,
    pub github_enabled: bool,
    pub managed_telegram_setup_enabled: bool,
    pub native_bot_mentions_writes_enabled: bool,
    /// Allows new thread contracts only after every reader supports them.
    pub thread_contract_writes_enabled: bool,
    pub multi_consumer_writes_enabled: bool,
    pub profile_revisions_enabled: bool,
    pub dynamic_sources_writes_enabled: bool,
    pub relay_raw_verification_writes_enabled: bool,
    pub lark_resource_writes_enabled: bool,
    pub native_lark_writes_enabled: bool,
    pub native_lark_intake_enabled: bool,
    pub native_lark_media_enabled: bool,
    pub native_lark_stickers_enabled: bool,
    pub native_whatsapp_groups_writes_enabled: bool,
    pub native_whatsapp_groups_enabled: bool,
    pub native_split_text_refs_enabled: bool,
    pub native_telegram_media_edits_enabled: bool,
    pub native_telegram_media_controls_enabled: bool,
    pub native_telegram_media_control_edits_enabled: bool,
    pub native_discord_media_edits_enabled: bool,
    pub native_discord_media_controls_enabled: bool,
    pub native_discord_media_card_edits_enabled: bool,
    pub native_discord_media_card_replacements_enabled: bool,
    pub native_discord_media_card_sets_enabled: bool,
    pub native_lark_controls_enabled: bool,
    pub native_lark_forms_enabled: bool,
    pub native_lark_message_effects_enabled: bool,
    pub native_lark_received_reactions_enabled: bool,
    pub native_whatsapp_message_events_enabled: bool,
    pub native_whatsapp_forms_enabled: bool,
    pub native_whatsapp_flow_writes_enabled: bool,
    pub native_typing_enabled: bool,
    pub native_whatsapp_writes_enabled: bool,
    pub native_whatsapp_media_enabled: bool,
    pub native_lark_outbound_media_enabled: bool,
    pub native_lark_sticker_replies_enabled: bool,
    pub native_lark_media_controls_enabled: bool,
    pub native_lark_multi_image_controls_enabled: bool,
    pub native_lark_media_card_replacements_enabled: bool,
    pub native_media_presentations_enabled: bool,
    pub native_discord_voice_enabled: bool,
    pub native_telegram_structured_replies_enabled: bool,
    pub native_whatsapp_structured_replies_enabled: bool,
    pub native_telegram_albums_enabled: bool,
    /// Start an exact admitted root thread with one explicit text reply.
    pub native_lark_thread_start_enabled: bool,
    pub native_whatsapp_outbound_media_enabled: bool,
    pub native_whatsapp_interactions_enabled: bool,
    pub native_whatsapp_media_controls_enabled: bool,
    pub native_whatsapp_reactions_enabled: bool,
    pub native_telegram_received_reactions_enabled: bool,
    pub native_discord_received_reactions_enabled: bool,
    pub native_whatsapp_templates_enabled: bool,
    pub lark_message_effects_writes_enabled: bool,
    pub native_discord_writes_enabled: bool,
    pub native_telegram_message_events_writes_enabled: bool,
    pub native_telegram_member_joins_enabled: bool,
    pub relay_telegram_member_joins_enabled: bool,
    pub native_discord_thread_history_enabled: bool,
    pub native_telegram_topic_lifecycle_enabled: bool,
    pub native_telegram_topic_creation_enabled: bool,
    pub native_telegram_general_topic_lifecycle_enabled: bool,
    pub native_discord_thread_lifecycle_enabled: bool,
    pub native_lark_conversation_history_enabled: bool,
    pub native_lark_mixed_history_enabled: bool,
    pub native_discord_conversation_history_enabled: bool,
    pub native_discord_reactions_enabled: bool,
    pub native_discord: NativeDiscordConfig,
    pub outbound_messages_writes_enabled: bool,
    pub event_side_messages_enabled: bool,
    pub ingress: IngressConfig,
    pub turns: TurnConfig,
    pub delivery: DeliveryConfig,
    pub media: MediaConfig,
    pub routing: RoutingConfig,
    pub stt: SttConfig,
    pub transformer: TransformerConfig,
    pub console: ConsoleConfig,
}

impl Config {
    /// Read configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        crate::telemetry::turn_trace::config::enabled()?;
        let value = env::var(CMAEG_LISTEN).unwrap_or_else(|_| "0.0.0.0:8090".to_owned());
        let listen = value.parse().map_err(|source| ConfigError::InvalidListen {
            name: CMAEG_LISTEN,
            value,
            source,
        })?;
        let value = env::var(CMAEG_METRICS_LISTEN).unwrap_or_else(|_| "0.0.0.0:9091".to_owned());
        let metrics_listen = value.parse().map_err(|source| ConfigError::InvalidListen {
            name: CMAEG_METRICS_LISTEN,
            value,
            source,
        })?;
        let providers_json = env::var(CMAEG_PROVIDERS).unwrap_or_else(|_| "[]".to_owned());
        let providers = ProviderRegistry::parse(&providers_json)?;
        // Validated against the registry, so it can only follow it.
        let media = media::config_from_env(&providers)?;
        let routing = routing::config_from_env(&providers)?;
        let stt = stt::config_from_env()?;
        let transformer = transformer::config_from_env()?;
        transformer.check(&media, &stt)?;
        let provider_connect_timeout = positive_duration(CMAEG_PROVIDER_CONNECT_TIMEOUT_MS, 2_000)?;
        let provider_request_timeout =
            positive_duration(CMAEG_PROVIDER_REQUEST_TIMEOUT_MS, 30_000)?;
        let provider_max_inflight = env::var(CMAEG_PROVIDER_MAX_INFLIGHT)
            .unwrap_or_else(|_| "256".to_owned())
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| ConfigError::InvalidMaxInflight {
                value: env::var(CMAEG_PROVIDER_MAX_INFLIGHT).unwrap_or_else(|_| "256".to_owned()),
            })?;
        let management = ManagementConfig::from_env()?;
        let turns = TurnConfig::from_env()?;
        media.check_turn_timeout(turns.turn_timeout)?;
        Ok(Self {
            listen,
            metrics_listen,
            providers,
            provider_connect_timeout,
            provider_request_timeout,
            provider_max_inflight,
            management,
            cma_service_account: ServiceAccountConfig::from_env()?,
            channel_cache: ChannelCacheConfig::from_env()?,
            github_enabled: github_writes_enabled()?,
            native_bot_mentions_writes_enabled: routing::write_flag(
                "CMAEG_NATIVE_BOT_MENTIONS_WRITES_ENABLED",
            )?,
            thread_contract_writes_enabled: routing::write_flag(
                "CMAEG_THREAD_CONTRACT_WRITES_ENABLED",
            )?,
            profile_revisions_enabled: routing::write_flag(
                "CMAEG_CHANNEL_PROFILE_REVISIONS_ENABLED",
            )?,
            multi_consumer_writes_enabled: routing::write_flag(
                "CMAEG_MULTI_CONSUMER_WRITES_ENABLED",
            )?,
            dynamic_sources_writes_enabled: routing::write_flag(
                "CMAEG_DYNAMIC_SOURCES_WRITES_ENABLED",
            )?,
            native_discord_reactions_enabled: routing::write_flag(
                "CMAEG_DISCORD_REACTIONS_ENABLED",
            )?,
            native_lark_conversation_history_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_CONVERSATION_HISTORY_ENABLED",
            )?,
            native_lark_mixed_history_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_MIXED_HISTORY_ENABLED",
            )?,
            native_discord_conversation_history_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_CONVERSATION_HISTORY_ENABLED",
            )?,
            native_telegram_topic_lifecycle_enabled: routing::write_flag(
                "CMAEG_NATIVE_TELEGRAM_TOPIC_LIFECYCLE_ENABLED",
            )?,
            native_telegram_topic_creation_enabled: routing::write_flag(
                "CMAEG_NATIVE_TELEGRAM_TOPIC_CREATION_ENABLED",
            )?,
            native_telegram_general_topic_lifecycle_enabled: routing::write_flag(
                "CMAEG_NATIVE_TELEGRAM_GENERAL_TOPIC_LIFECYCLE_ENABLED",
            )?,
            native_discord_thread_lifecycle_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_THREAD_LIFECYCLE_ENABLED",
            )?,
            native_discord_thread_history_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_THREAD_HISTORY_ENABLED",
            )?,
            native_telegram_message_events_writes_enabled: routing::write_flag(
                "CMAEG_NATIVE_TELEGRAM_MESSAGE_EVENTS_WRITES_ENABLED",
            )?,
            native_telegram_member_joins_enabled: routing::write_flag(
                "CMAEG_NATIVE_TELEGRAM_MEMBER_JOINS_ENABLED",
            )?,
            relay_telegram_member_joins_enabled: routing::write_flag(
                "CMAEG_RELAY_TELEGRAM_MEMBER_JOINS_ENABLED",
            )?,
            managed_telegram_setup_enabled: routing::write_flag(
                "CMAEG_MANAGED_TELEGRAM_SETUP_ENABLED",
            )?,
            native_lark_writes_enabled: routing::write_flag("CMAEG_NATIVE_LARK_WRITES_ENABLED")?,
            native_lark_intake_enabled: routing::write_flag("CMAEG_NATIVE_LARK_INTAKE_ENABLED")?,
            native_lark_media_enabled: routing::write_flag("CMAEG_NATIVE_LARK_MEDIA_ENABLED")?,
            native_lark_stickers_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_STICKERS_ENABLED",
            )?,
            native_whatsapp_groups_writes_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_GROUPS_WRITES_ENABLED",
            )?,
            native_whatsapp_groups_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_GROUPS_ENABLED",
            )?,
            native_telegram_media_control_edits_enabled: routing::write_flag(
                "CMAEG_NATIVE_TELEGRAM_MEDIA_CONTROL_EDITS_ENABLED",
            )?,
            native_telegram_media_controls_enabled: routing::write_flag(
                "CMAEG_NATIVE_TELEGRAM_MEDIA_CONTROLS_ENABLED",
            )?,
            native_split_text_refs_enabled: routing::write_flag(
                "CMAEG_NATIVE_SPLIT_TEXT_REFS_ENABLED",
            )?,
            native_telegram_media_edits_enabled: routing::write_flag(
                "CMAEG_NATIVE_TELEGRAM_MEDIA_EDITS_ENABLED",
            )?,
            native_discord_media_controls_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_MEDIA_CONTROLS_ENABLED",
            )?,
            native_discord_media_card_edits_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_MEDIA_CARD_EDITS_ENABLED",
            )?,
            native_discord_media_card_sets_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_MEDIA_CARD_SETS_ENABLED",
            )?,
            native_discord_media_card_replacements_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_MEDIA_CARD_REPLACEMENTS_ENABLED",
            )?,
            native_discord_media_edits_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_MEDIA_EDITS_ENABLED",
            )?,
            native_lark_controls_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_CONTROLS_ENABLED",
            )?,
            native_lark_forms_enabled: routing::write_flag("CMAEG_NATIVE_LARK_FORMS_ENABLED")?,
            native_lark_message_effects_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_MESSAGE_EFFECTS_ENABLED",
            )?,
            native_lark_received_reactions_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_RECEIVED_REACTIONS_ENABLED",
            )?,
            native_whatsapp_message_events_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_MESSAGE_EVENTS_ENABLED",
            )?,
            native_whatsapp_forms_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_FORMS_ENABLED",
            )?,
            native_whatsapp_flow_writes_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_FLOW_WRITES_ENABLED",
            )?,
            native_typing_enabled: routing::write_flag("CMAEG_NATIVE_TYPING_ENABLED")?,
            native_whatsapp_writes_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_WRITES_ENABLED",
            )?,
            native_lark_thread_start_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_THREAD_START_ENABLED",
            )?,
            native_lark_media_controls_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_MEDIA_CONTROLS_ENABLED",
            )?,
            native_discord_voice_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_VOICE_ENABLED",
            )?,
            native_media_presentations_enabled: routing::write_flag(
                "CMAEG_NATIVE_MEDIA_PRESENTATIONS_ENABLED",
            )?,
            native_telegram_structured_replies_enabled: routing::write_flag(
                "CMAEG_NATIVE_TELEGRAM_STRUCTURED_REPLIES_ENABLED",
            )?,
            native_whatsapp_structured_replies_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_STRUCTURED_REPLIES_ENABLED",
            )?,
            native_telegram_albums_enabled: routing::write_flag(
                "CMAEG_NATIVE_TELEGRAM_ALBUMS_ENABLED",
            )?,
            native_lark_multi_image_controls_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_MULTI_IMAGE_CONTROLS_ENABLED",
            )?,
            native_lark_media_card_replacements_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_MEDIA_CARD_REPLACEMENTS_ENABLED",
            )?,
            native_lark_outbound_media_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_OUTBOUND_MEDIA_ENABLED",
            )?,
            native_lark_sticker_replies_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_STICKER_REPLIES_ENABLED",
            )?,
            native_whatsapp_outbound_media_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_OUTBOUND_MEDIA_ENABLED",
            )?,
            native_whatsapp_interactions_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_INTERACTIONS_ENABLED",
            )?,
            native_whatsapp_media_controls_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_MEDIA_CONTROLS_ENABLED",
            )?,
            native_whatsapp_reactions_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_REACTIONS_ENABLED",
            )?,
            native_telegram_received_reactions_enabled: routing::telegram_received_reactions()?,
            native_discord_received_reactions_enabled: routing::discord_received_reactions()?,
            native_whatsapp_templates_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_TEMPLATES_ENABLED",
            )?,
            native_whatsapp_media_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_MEDIA_ENABLED",
            )?,
            native_discord_writes_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_WRITES_ENABLED",
            )?,
            relay_raw_verification_writes_enabled: routing::write_flag(
                "CMAEG_RELAY_RAW_VERIFICATION_WRITES_ENABLED",
            )?,
            lark_message_effects_writes_enabled: routing::write_flag(
                "CMAEG_LARK_MESSAGE_EFFECTS_WRITES_ENABLED",
            )?,
            lark_resource_writes_enabled: routing::write_flag(
                "CMAEG_LARK_RESOURCE_WRITES_ENABLED",
            )?,
            outbound_messages_writes_enabled: routing::write_flag(
                "CMAEG_OUTBOUND_MESSAGES_WRITES_ENABLED",
            )?,
            event_side_messages_enabled: routing::write_flag("CMAEG_EVENT_SIDE_MESSAGES_ENABLED")?,
            ingress: IngressConfig::from_env()?,
            turns,
            delivery: DeliveryConfig::from_env()?,
            media,
            native_discord: native_discord::config_from_env(&routing)?,
            routing,
            stt,
            transformer,
            console: ConsoleConfig::from_env()?,
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod public_api_tests;
