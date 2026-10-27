//! One tool selection for MCP and authenticated owner discovery.
use super::{
    message_effects,
    tools::{self, ToolSpec},
};
use crate::tools::ToolsState;
use serde::Serialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

/// Server tool metadata. Availability does not grant channel, source or event authority.
#[derive(Serialize, ToSchema)]
pub struct AgentToolCapability {
    pub name: &'static str,
    pub description: &'static str,
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

pub(super) struct SelectedTool {
    pub spec: &'static ToolSpec,
    pub definitions: fn() -> Value,
}
impl SelectedTool {
    pub fn describe(&self) -> Value {
        tools::describe(self.spec, &(self.definitions)())
    }
    fn capability(&self) -> AgentToolCapability {
        let tool = self.spec;
        AgentToolCapability {
            name: tool.name,
            description: tool.description,
            read_only: tool.read_only,
            destructive: tool.destructive,
            idempotent: tool.idempotent,
            open_world: tool.open_world,
        }
    }
}

fn selected(state: &ToolsState) -> Vec<SelectedTool> {
    let mut selected = tools::selected(state.media.is_some());
    let effects = state
        .native_media_edits
        .as_ref()
        .is_some_and(|c| c.telegram_issuance || c.discord_issuance)
        || state
            .whatsapp_reactions
            .as_ref()
            .is_some_and(|client| client.issuance_enabled)
        || state.native_lark_effects.as_ref().is_some_and(|c| {
            c.confirmed_issuance_enabled
                || c.received_issuance_enabled
                || c.controls_issuance_enabled
        })
        || (state.lark_effects_issuance_enabled && state.lark_effects.is_some())
        || state.received_reaction_enabled(
            cmaeg_core::seal::received_reaction::ReceivedReactionPlatform::Telegram,
        )
        || state.received_reaction_enabled(
            cmaeg_core::seal::received_reaction::ReceivedReactionPlatform::Discord,
        )
        || state.telegram_effects_issuance_enabled
        || state.discord_effects_issuance_enabled
        || state
            .discord_effects
            .as_ref()
            .is_some_and(|client| client.card_effects_enabled());
    selected.extend(message_effects::selected(effects));
    selected
}

pub(crate) fn view(state: &ToolsState) -> Vec<AgentToolCapability> {
    selected(state)
        .iter()
        .map(SelectedTool::capability)
        .collect()
}

pub(super) fn list(state: &ToolsState) -> Value {
    json!({"tools": selected(state).iter().map(SelectedTool::describe).collect::<Vec<_>>()})
}
