//! Authenticated authoring discovery, independent from native connection setup.
use crate::channels::management::ManagementState;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct RelayAuthoringCapabilities {
    /// New Custom channels may opt into durable execution for recovery-capable providers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_durable_execution_enabled: Option<bool>,
    /// Registered provider slugs with response replay. Configuration, not connection health.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_recovery_providers: Option<Vec<String>>,
    /// Separate native custom app only; never replaces a NyxID webhook.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_lark_writes_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_lark_intake_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_lark_controls_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_lark_forms_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_telegram_message_events_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_telegram_member_joins_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_telegram_member_joins_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_discord_thread_history_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_telegram_topic_lifecycle_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_telegram_topic_creation_enabled: Option<bool>,
    /// General-topic execution additionally requires the original native forum envelope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_telegram_general_topic_lifecycle_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_discord_thread_lifecycle_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_discord_conversation_history_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_lark_conversation_history_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_discord_reactions_enabled: Option<bool>,
    /// Descriptive sticker/embed/poll intake, not media download or poll actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_discord_rich_content_enabled: Option<bool>,
    /// Bounded descriptions of received Discord forwards, without origin access.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_discord_forwards_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_discord_writes_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_discord_auto_activation_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_discord_capacity: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_whatsapp_writes_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_whatsapp_forms_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_whatsapp_message_events_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_whatsapp_flow_writes_enabled: Option<bool>,
    /// Owner opt-in to existing OBA groups; independent of group issuance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_whatsapp_groups_writes_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_whatsapp_auto_activation_enabled: Option<bool>,
    pub dynamic_sources_writes_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outbound_messages_writes_enabled: Option<bool>,
    /// The configured relay accepts new native attachments; platform limits still apply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_media_writes_enabled: Option<bool>,
    pub relay_raw_verification_writes_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lark_resource_writes_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lark_message_effects_writes_enabled: Option<bool>,
    pub multi_consumer_writes_enabled: bool,
    pub native_bot_mentions_writes_enabled: bool,
    pub thread_contract_writes_enabled: bool,
    pub platforms: Vec<RelayPlatformCapability>,
}
#[derive(Serialize, ToSchema)]
pub struct RelayPlatformCapability {
    pub id: String,
    pub label: String,
    pub verification_modes: Vec<RelayVerificationCapability>,
}
#[derive(Serialize, ToSchema)]
pub struct RelayVerificationCapability {
    /// native_event, raw_event or trusted_normalized. Not a delivery-health claim.
    pub id: String,
    /// Whether this deployment accepts new writes selecting this mode.
    /// Existing exact settings may be retained even while this is false.
    pub writes_enabled: bool,
}

pub(super) fn view(state: &ManagementState) -> RelayAuthoringCapabilities {
    let platforms = [
        ("telegram", "Telegram"),
        ("lark", "Lark"),
        ("feishu", "Feishu"),
        ("discord", "Discord"),
        ("whatsapp", "WhatsApp"),
        ("slack", "Slack"),
    ]
    .into_iter()
    .map(|(id, label)| RelayPlatformCapability {
        id: id.into(),
        label: label.into(),
        verification_modes: match id {
            "discord" | "whatsapp" => vec![
                mode("raw_event", state.relay_raw_verification_writes_enabled),
                mode("trusted_normalized", true),
            ],
            "slack" => vec![mode("trusted_normalized", true)],
            _ => vec![mode("native_event", true)],
        },
    })
    .collect();
    RelayAuthoringCapabilities {
        custom_durable_execution_enabled: Some(state.custom_durable_execution_enabled),
        response_recovery_providers: Some(state.providers.response_recovery_providers()),
        native_lark_writes_enabled: Some(
            state.native_lark_writes_enabled
                && state.native_lark_admission_enabled
                && state.multi_consumer_writes_enabled,
        ),
        native_lark_controls_enabled: Some(
            state.native_lark_controls_enabled
                && state.native_lark_intake_enabled
                && state.native_lark_admission_enabled,
        ),
        native_lark_forms_enabled: Some(
            state.native_lark_forms_enabled
                && state.native_lark_controls_enabled
                && state.native_lark_intake_enabled
                && state.native_lark_admission_enabled,
        ),
        native_lark_intake_enabled: Some(
            state.native_lark_intake_enabled && state.native_lark_admission_enabled,
        ),
        native_telegram_message_events_enabled: Some(
            state.native_telegram_message_events_writes_enabled
                && state.native_telegram_durable_admission_enabled
                && state.multi_consumer_writes_enabled,
        ),
        native_telegram_member_joins_enabled: Some(state.telegram_member_join_authoring()),
        relay_telegram_member_joins_enabled: Some(state.relay_telegram_member_join_authoring()),
        native_telegram_topic_lifecycle_enabled: Some(
            state.native_telegram_topic_lifecycle_enabled
                && state.native_telegram_durable_admission_enabled
                && state.multi_consumer_writes_enabled,
        ),
        native_telegram_topic_creation_enabled: Some(
            state.native_telegram_topic_creation_enabled
                && state.native_telegram_durable_admission_enabled
                && state.multi_consumer_writes_enabled,
        ),
        native_telegram_general_topic_lifecycle_enabled: Some(
            state.native_telegram_general_topic_lifecycle_enabled
                && state.native_telegram_topic_lifecycle_enabled
                && state.native_telegram_durable_admission_enabled
                && state.multi_consumer_writes_enabled,
        ),
        native_discord_thread_history_enabled: Some(state.native_discord_thread_history_enabled),
        native_lark_conversation_history_enabled: Some(
            state.native_lark_conversation_history_enabled,
        ),
        native_discord_conversation_history_enabled: Some(
            state.native_discord_conversation_history_enabled,
        ),
        native_discord_thread_lifecycle_enabled: Some(
            state.native_discord_thread_lifecycle_enabled,
        ),
        native_discord_reactions_enabled: Some(state.native_discord_reactions_enabled),
        native_discord_rich_content_enabled: Some(state.native_discord_rich_content_enabled),
        native_discord_forwards_enabled: Some(state.native_discord_forwards_enabled),
        native_discord_writes_enabled: Some(state.native_discord_writes_enabled),
        native_discord_auto_activation_enabled: Some(
            state.native_discord.discovery_enabled() && state.native_discord_admission_enabled,
        ),
        native_discord_capacity: Some(crate::discord::selection::MAX_RECORDS),
        native_whatsapp_writes_enabled: Some(state.native_whatsapp_writes_enabled),
        native_whatsapp_message_events_enabled: Some(
            state.native_whatsapp_message_events_enabled && state.native_whatsapp_admission_enabled,
        ),
        native_whatsapp_forms_enabled: Some(
            state.native_whatsapp_forms_enabled && state.native_whatsapp_admission_enabled,
        ),
        native_whatsapp_flow_writes_enabled: Some(
            state.native_whatsapp_flow_writes_enabled && state.native_whatsapp_admission_enabled,
        ),
        native_whatsapp_groups_writes_enabled: Some(
            state.native_whatsapp_groups_writes_enabled
                && state.native_whatsapp_writes_enabled
                && state.native_whatsapp_admission_enabled
                && state.multi_consumer_writes_enabled,
        ),
        native_whatsapp_auto_activation_enabled: Some(
            state.native_whatsapp_admission_enabled && state.multi_consumer_writes_enabled,
        ),
        dynamic_sources_writes_enabled: state.dynamic_sources_writes_enabled,
        outbound_messages_writes_enabled: Some(state.outbound_messages_writes_enabled),
        native_media_writes_enabled: Some(
            state
                .outbound_relay
                .as_ref()
                .is_some_and(|relay| relay.native_media_enabled()),
        ),
        relay_raw_verification_writes_enabled: state.relay_raw_verification_writes_enabled,
        lark_resource_writes_enabled: Some(state.lark_resource_writes_enabled),
        lark_message_effects_writes_enabled: Some(state.lark_message_effects_writes_enabled),
        multi_consumer_writes_enabled: state.multi_consumer_writes_enabled,
        native_bot_mentions_writes_enabled: state.native_bot_mentions_writes_enabled,
        thread_contract_writes_enabled: state.thread_contract_writes_enabled,
        platforms,
    }
}
fn mode(id: &str, writes_enabled: bool) -> RelayVerificationCapability {
    RelayVerificationCapability {
        id: id.into(),
        writes_enabled,
    }
}
