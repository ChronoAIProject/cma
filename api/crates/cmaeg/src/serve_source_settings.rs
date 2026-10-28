//! Native source configuration and optional persistent-store readiness.
use crate::{http::ManagementState, Config};

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
