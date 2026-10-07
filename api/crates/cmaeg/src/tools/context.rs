use super::{
    attachment_view::{InboundMediaCapabilities, VisualSupport},
    ToolsState,
};
use crate::{channels::resolver::ResolvedChannel, media::visual::VisualKind};
use cmaeg_core::{
    activity::{Activity, ActivityConversation, ActivityKind, Actor, SourceKind},
    channel::Platform,
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
    pub expires_at: i64,
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
    /// What this binding can do with inbound attachments; absent while
    /// inbound media is disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbound_media: Option<InboundMediaCapabilities>,
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
            inbound_media: None,
        }
    }
    pub fn native_telegram() -> Self {
        Self {
            typing: true,
            ..Self::telegram()
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
            inbound_media: None,
        }
    }
    /// Text-only delivery through NyxID's generic relay reply: no native
    /// controls or message updates, so choices degrade to numbered text.
    pub fn plain_text() -> Self {
        Self {
            edit: false,
            typing: false,
            buttons: "numbered_text".into(),
            cards: "text".into(),
            attachments: "link_text".into(),
            max_text_chars: 4000,
            inbound_media: None,
        }
    }
    /// Interactive controls and updates are proven only for Telegram.
    pub fn relay(platform: Platform) -> Self {
        match platform {
            Platform::Telegram => Self::telegram(),
            _ => Self::plain_text(),
        }
    }
}
pub(super) async fn read(
    state: &ToolsState,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
) -> Result<ContextResponse, Problem> {
    let (activity, activity_verified) = read_activity(state, channel, event).await?;
    let inbound_media = inbound_media(state, channel, &activity.source);
    Ok(project(
        channel,
        event,
        activity,
        activity_verified,
        inbound_media,
    ))
}

/// The inbound media capabilities of `channel`'s binding for an activity
/// from `source`, or `None` while media is disabled, so that the context
/// output is unchanged with the flag off.
pub(super) fn inbound_media(
    state: &ToolsState,
    channel: &ResolvedChannel,
    source: &SourceKind,
) -> Option<InboundMediaCapabilities> {
    let media = state.media.as_ref()?;
    let capabilities = media.capabilities();
    let visuals = match (capabilities.visuals, source) {
        (VisualKind::Keyframes, _) => VisualSupport::Keyframes,
        (VisualKind::Thumbnail, SourceKind::Telegram { .. }) => VisualSupport::PlatformThumbnail,
        (VisualKind::Thumbnail, _) => VisualSupport::None,
    };
    Some(InboundMediaCapabilities {
        read_tool: media.audience(&channel.definition).tools,
        transcripts: capabilities.transcriber_enabled,
        visuals,
        // Set from Audience::downloads by media-download-links once links
        // exist; never advertise an unimplemented ability.
        downloads: false,
    })
}

/// The provider's persisted activity for `event` and whether the event_ref's
/// sealed digest verifies it.
///
/// An event_ref sealed without a digest yields an unverified activity; callers
/// that act on the activity's contents must refuse that themselves.
pub(super) async fn read_activity(
    state: &ToolsState,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
) -> Result<(Activity, bool), Problem> {
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
    Ok((snapshot.activity, activity_verified))
}

pub(super) fn project(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: Activity,
    activity_verified: bool,
    inbound_media: Option<InboundMediaCapabilities>,
) -> ContextResponse {
    let capabilities = Capabilities {
        inbound_media,
        ..match activity.source {
            SourceKind::Telegram { .. } => Capabilities::native_telegram(),
            SourceKind::Custom => Capabilities::custom(),
            SourceKind::NyxidRelay { platform, .. } => Capabilities::relay(platform),
        }
    };
    ContextResponse {
        event_id: event.event_id.clone(),
        channel_id: channel.id.clone(),
        provider_binding_id: channel.definition.provider.binding_id.clone(),
        source: activity.source.clone(),
        activity_verified,
        expires_at: event.expires_at,
        conversation_id: event.conversation_id.clone(),
        activity: PublicActivity {
            kind: activity.kind,
            actor: activity.actor,
            conversation: activity.conversation,
            occurred_at: activity.occurred_at,
        },
        capabilities,
    }
}
fn unavailable() -> Problem {
    Problem::new(
        ProblemCode::ProviderUnavailable,
        503,
        "event_context_unavailable",
    )
}
