//! Native source configuration and optional persistent-store readiness.
use crate::{http::ManagementState, Config};
use std::sync::Arc;

pub(super) fn lark_issuance(config: &Config, state: &ManagementState) -> (bool, bool) {
    let admitted = config.native_lark_intake_enabled && state.native_lark_admission_enabled;
    (
        admitted && config.native_lark_controls_enabled,
        admitted && config.native_lark_thread_start_enabled,
    )
}

/// Keep native card/effect client construction and its issuance gates together.
pub(super) fn lark_effects(
    config: &Config,
    controls: bool,
) -> Result<Arc<crate::tools::NativeLarkEffectsClient>, crate::ConfigError> {
    Ok(Arc::new(
        crate::tools::NativeLarkEffectsClient::new(
            crate::native_lark::Client::production()?,
            config.native_lark_message_effects_enabled,
            config.native_lark_received_reactions_enabled,
        )
        .with_controls(controls, config.native_lark_forms_enabled),
    ))
}

/// Build only enabled native history readers; preserve startup failure on invalid clients.
pub(super) fn lark_history(
    enabled: bool,
    mixed_media: bool,
) -> Result<
    Option<Arc<crate::tools::NativeLarkConversationHistory>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    Ok(enabled
        .then(|| {
            crate::native_lark::Client::history_reader().map(|api| {
                Arc::new(crate::tools::NativeLarkConversationHistory::new(
                    api,
                    mixed_media,
                ))
            })
        })
        .transpose()?)
}

pub(super) fn discord_history(
    enabled: bool,
) -> Result<
    Option<Arc<crate::tools::DiscordConversationHistory>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    Ok(enabled
        .then(|| {
            crate::discord::DiscordRest::new()
                .map(crate::tools::DiscordConversationHistory::new)
                .map(Arc::new)
        })
        .transpose()?)
}

/// Return Forms storage readiness independently of issuance so recovery can run.
pub(super) async fn configure(
    config: &Config,
    state: &mut ManagementState,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    state.native_bot_mentions_writes_enabled = config.native_bot_mentions_writes_enabled;
    state.thread_contract_writes_enabled = config.thread_contract_writes_enabled;
    state.multi_consumer_writes_enabled = config.multi_consumer_writes_enabled;
    state.dynamic_sources_writes_enabled = config.dynamic_sources_writes_enabled;
    state.relay_raw_verification_writes_enabled = config.relay_raw_verification_writes_enabled;
    state.lark_resource_writes_enabled = config.lark_resource_writes_enabled;
    state.native_lark_intake_enabled = config.native_lark_intake_enabled;
    state.native_lark_controls_enabled = config.native_lark_controls_enabled;
    state.native_lark_forms_enabled = config.native_lark_forms_enabled;
    state.native_lark_writes_enabled = config.native_lark_writes_enabled;
    state.native_lark_admission_enabled =
        config.routing.admission_enabled && config.routing.recovery_enabled;
    state.native_lark = Some(std::sync::Arc::new(
        crate::native_lark::Client::production()?
    ));
    let whatsapp_forms_ready = state.records.whatsapp_flows_ready().await;
    state.native_whatsapp_forms_enabled =
        config.native_whatsapp_forms_enabled && whatsapp_forms_ready;
    state.native_whatsapp_flow_writes_enabled =
        config.native_whatsapp_flow_writes_enabled && whatsapp_forms_ready;
    state.native_whatsapp_message_events_enabled = config.native_whatsapp_message_events_enabled;
    state.native_whatsapp_writes_enabled = config.native_whatsapp_writes_enabled;
    state.native_whatsapp_groups_writes_enabled = config.native_whatsapp_groups_writes_enabled;
    state.native_whatsapp_admission_enabled =
        config.routing.admission_enabled && config.routing.recovery_enabled;
    state.lark_message_effects_writes_enabled = config.lark_message_effects_writes_enabled;
    state.native_discord_writes_enabled = config.native_discord_writes_enabled;
    state.native_telegram_message_events_writes_enabled =
        config.native_telegram_message_events_writes_enabled;
    state.native_telegram_durable_admission_enabled =
        config.routing.recovery_enabled && config.routing.admission_enabled;
    state.native_discord_thread_history_enabled = config.native_discord_thread_history_enabled;
    state.native_telegram_topic_lifecycle_enabled = config.native_telegram_topic_lifecycle_enabled
        && state.native_telegram_durable_admission_enabled
        && state.records.thread_fences_ready().await;
    state.native_telegram_general_topic_lifecycle_enabled = config
        .native_telegram_general_topic_lifecycle_enabled
        && state.native_telegram_topic_lifecycle_enabled;
    state.native_discord_thread_lifecycle_enabled =
        config.native_discord_thread_lifecycle_enabled && state.records.thread_fences_ready().await;
    state.native_lark_conversation_history_enabled =
        config.native_lark_conversation_history_enabled;
    state.native_discord_conversation_history_enabled =
        config.native_discord_conversation_history_enabled;
    state.native_discord_reactions_enabled = config.native_discord_reactions_enabled;
    state.native_discord = crate::discord::selection::Selection::new(&config.native_discord);
    state.native_discord_admission_enabled = config.routing.admission_enabled;
    Ok(whatsapp_forms_ready)
}

impl crate::delivery::relay_client::RelayClient {
    pub(super) fn with_native_whatsapp_config(self, config: &Config) -> Self {
        self.with_native_whatsapp_outbound_media(config.native_whatsapp_outbound_media_enabled)
            .with_native_whatsapp_templates(config.native_whatsapp_templates_enabled)
            .with_native_whatsapp_interactions(config.native_whatsapp_interactions_enabled)
            .with_native_whatsapp_media_controls(config.native_whatsapp_media_controls_enabled)
            .with_native_whatsapp_groups(config.native_whatsapp_groups_enabled)
    }
}

/// Compose topic execution only after the effective persistent-store gates are set.
pub(super) fn telegram_topics(
    state: &ManagementState,
    api: Arc<crate::telegram::TelegramClient>,
) -> Option<Arc<crate::tools::TelegramTopics>> {
    state.native_telegram_topic_lifecycle_enabled.then(|| {
        Arc::new(
            crate::tools::TelegramTopics::new(api)
                .with_general_topics(state.native_telegram_general_topic_lifecycle_enabled),
        )
    })
}
