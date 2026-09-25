use super::ToolsState;
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::{ActivityConversation, ActivityKind, Actor},
    ids::{ConversationId, EventId},
    problem::{Problem, ProblemCode},
    seal::EventRefPayload,
};
use serde::Serialize;
use utoipa::ToSchema;
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ContextResponse {
    pub event_id: EventId,
    pub channel_id: cmaeg_core::ids::ChannelId,
    pub provider_binding_id: cmaeg_core::ids::BindingId,
    pub source: cmaeg_core::activity::SourceKind,
    pub activity_verified: bool,
    pub conversation_id: ConversationId,
    pub activity: PublicActivity,
    pub capabilities: Capabilities,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PublicActivity {
    pub kind: ActivityKind,
    pub actor: Actor,
    pub conversation: ActivityConversation,
    pub occurred_at: String,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Capabilities {
    pub edit: bool,
    pub typing: bool,
    pub buttons: String,
    pub cards: String,
    pub attachments: String,
    pub max_text_chars: u32,
}
impl Capabilities {
    pub fn custom() -> Self {
        Self {
            edit: false,
            typing: false,
            buttons: "json".into(),
            cards: "json".into(),
            attachments: "reference".into(),
            max_text_chars: 32768,
        }
    }
    pub fn telegram() -> Self {
        Self {
            edit: true,
            typing: false,
            buttons: "inline_keyboard".into(),
            cards: "text".into(),
            attachments: "link_text".into(),
            max_text_chars: 4000,
        }
    }
}
pub(super) async fn read(
    state: &ToolsState,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
) -> Result<ContextResponse, Problem> {
    let credential = channel
        .definition
        .secrets
        .provider_credential
        .as_ref()
        .ok_or_else(unavailable)?;
    let snapshot = state
        .providers
        .event_context(
            &channel.definition.provider.slug,
            &channel.definition.provider.binding_id,
            &event.conversation_id,
            &event.event_id,
            credential,
        )
        .await
        .map_err(|_| unavailable())?;
    if snapshot.channel_id != channel.id {
        return Err(unavailable());
    }
    let activity_verified = event.verifies_activity(&snapshot.activity);
    if event.activity_sha256.is_some() && !activity_verified {
        return Err(unavailable());
    }
    let activity = snapshot.activity;
    let capabilities = if matches!(activity.source, cmaeg_core::activity::SourceKind::Custom) {
        Capabilities::custom()
    } else {
        Capabilities::telegram()
    };
    Ok(ContextResponse {
        event_id: event.event_id.clone(),
        channel_id: channel.id.clone(),
        provider_binding_id: channel.definition.provider.binding_id.clone(),
        source: activity.source.clone(),
        activity_verified,
        conversation_id: event.conversation_id.clone(),
        activity: PublicActivity {
            kind: activity.kind,
            actor: activity.actor,
            conversation: activity.conversation,
            occurred_at: activity.occurred_at,
        },
        capabilities,
    })
}
fn unavailable() -> Problem {
    Problem::new(
        ProblemCode::ProviderUnavailable,
        503,
        "event_context_unavailable",
    )
}
