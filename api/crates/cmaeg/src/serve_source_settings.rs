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
) -> Result<(Arc<crate::telegram::TelegramClient>, bool), Box<dyn std::error::Error + Send + Sync>>
{
    let telegram = Arc::new(crate::telegram::TelegramClient::production(
        config.delivery.reply_timeout,
    )?);
    state.telegram = Some(telegram.clone());
    state.media_enabled = config.media.enabled;
    state.github_enabled = config.github_enabled;
    state.native_bot_mentions_writes_enabled = config.native_bot_mentions_writes_enabled;
    state.thread_contract_writes_enabled = config.thread_contract_writes_enabled;
    state.multi_consumer_writes_enabled = config.multi_consumer_writes_enabled;
    state.custom_durable_execution_enabled = config.routing.custom_durable_execution_enabled
        && config.routing.recovery_enabled
        && config.routing.admission_enabled
        && config.multi_consumer_writes_enabled;
    state.dynamic_sources_writes_enabled = config.dynamic_sources_writes_enabled;
    state.relay_raw_verification_writes_enabled = config.relay_raw_verification_writes_enabled;
    state.lark_resource_writes_enabled = config.lark_resource_writes_enabled;
    state.managed_telegram_setup_enabled = config.managed_telegram_setup_enabled;
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
    state.native_telegram_member_joins_enabled = config.native_telegram_member_joins_enabled;
    state.relay_telegram_member_joins_enabled = config.relay_telegram_member_joins_enabled;
    state.native_telegram_message_events_writes_enabled =
        config.native_telegram_message_events_writes_enabled;
    state.native_telegram_durable_admission_enabled =
        config.routing.recovery_enabled && config.routing.admission_enabled;
    state.native_discord_thread_history_enabled = config.native_discord_thread_history_enabled;
    state.native_telegram_topic_lifecycle_enabled = config.native_telegram_topic_lifecycle_enabled
        && state.native_telegram_durable_admission_enabled
        && state.records.thread_fences_ready().await;
    state.native_telegram_topic_creation_enabled = config.native_telegram_topic_creation_enabled
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
    state.native_discord_rich_content_enabled = config.native_discord.rich_content_enabled;
    state.native_discord_forwards_enabled = config.native_discord.forwards_enabled;
    state.native_discord = crate::discord::selection::Selection::new(&config.native_discord);
    state.native_discord_admission_enabled = config.routing.admission_enabled;
    Ok((telegram, whatsapp_forms_ready))
}

impl crate::delivery::relay_client::RelayClient {
    /// Compose native media presentations while preserving each independent issuance gate.
    pub(super) fn with_native_presentation_config(self, config: &Config) -> Self {
        self.with_native_media_presentations(config.native_media_presentations_enabled)
            .with_native_discord_voice(config.native_discord_voice_enabled)
            .with_native_telegram_albums(config.native_telegram_albums_enabled)
            .with_native_whatsapp_structured_replies(
                config.native_whatsapp_structured_replies_enabled
                    && config.native_whatsapp_writes_enabled,
            )
            .with_native_telegram_structured_replies(
                config.native_telegram_structured_replies_enabled,
            )
    }

    pub(super) fn with_native_lark_config(
        self,
        config: &Config,
        controls: bool,
        thread_start: bool,
    ) -> Self {
        self.with_native_lark_outbound_media(config.native_lark_outbound_media_enabled)
            .with_native_lark_sticker_replies(config.native_lark_sticker_replies_enabled)
            .with_native_lark_effects(config.native_lark_message_effects_enabled)
            .with_native_lark_controls(controls, config.native_lark_forms_enabled)
            .with_native_lark_media_controls(config.native_lark_media_controls_enabled)
            .with_native_lark_multi_image_controls(config.native_lark_multi_image_controls_enabled)
            .with_native_lark_media_card_replacements(
                config.native_lark_media_card_replacements_enabled,
            )
            .with_native_lark_thread_start(thread_start)
    }

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
    (state.native_telegram_topic_lifecycle_enabled || state.native_telegram_topic_creation_enabled)
        .then(|| {
            Arc::new(
                crate::tools::TelegramTopics::new(api)
                    .with_lifecycle(state.native_telegram_topic_lifecycle_enabled)
                    .with_creation(state.native_telegram_topic_creation_enabled)
                    .with_general_topics(state.native_telegram_general_topic_lifecycle_enabled),
            )
        })
}

/// Typing is an explicit, short-lived native write, separately activated for rollout.
pub(super) fn native_typing(
    config: &Config,
    state: &ManagementState,
) -> Result<Option<Arc<crate::tools::NativeTyping>>, crate::ConfigError> {
    if !config.native_typing_enabled {
        return Ok(None);
    }
    Ok(Some(Arc::new(crate::tools::NativeTyping::new(
        crate::whatsapp::Client::production()?,
        crate::discord::DiscordRest::new()?,
        state.native_whatsapp_admission_enabled && state.native_whatsapp_writes_enabled,
        state.native_discord_admission_enabled
            && state.native_discord_writes_enabled
            && config.native_discord.enabled
            && config.routing.recovery_enabled,
    ))))
}

impl crate::ingress::IngressState {
    /// Preserve independent intake gates while composing all native source settings.
    pub(super) fn with_native_intake_config(mut self, config: &Config) -> Self {
        self.native_telegram_member_joins_enabled = config.native_telegram_member_joins_enabled;
        self.relay_telegram_member_joins_enabled = config.relay_telegram_member_joins_enabled;
        self.with_media_enabled(config.media.enabled)
            .with_native_lark_intake(config.native_lark_intake_enabled)
            .with_native_lark_media(config.native_lark_media_enabled)
            .with_native_lark_stickers(config.native_lark_stickers_enabled)
            .with_whatsapp_message_events(config.native_whatsapp_message_events_enabled)
            .with_whatsapp_media(config.native_whatsapp_media_enabled)
            .with_whatsapp_groups(config.native_whatsapp_groups_enabled)
    }
}
