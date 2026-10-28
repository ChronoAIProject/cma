use super::{
    attachment_view::{InboundMediaCapabilities, VisualSupport},
    ToolsState,
};
use crate::{channels::resolver::ResolvedChannel, media::visual::VisualKind};
use cmaeg_core::{
    activity::{Activity, ActivityConversation, ActivityKind, ActivityRouting, Actor, SourceKind},
    channel::{ConsumerView, Platform},
    ids::{ConsumerId, ConsumerName, ConversationId, EventId},
    problem::{Problem, ProblemCode},
    seal::EventRefPayload,
};
use serde::Serialize;
#[path = "context_native.rs"]
mod native;
use utoipa::ToSchema;
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ContextResponse {
    /// Reaction-only reference to an exact retained native user message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_message: Option<super::ReceivedMessageContext>,
    /// Explicitly requested current text context of the original native thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_history: Option<super::ThreadHistory>,
    pub event_id: EventId,
    pub channel_id: cmaeg_core::ids::ChannelId,
    pub provider_binding_id: cmaeg_core::ids::BindingId,
    pub source: cmaeg_core::activity::SourceKind,
    pub activity_verified: bool,
    pub expires_at: i64,
    pub conversation_id: ConversationId,
    pub activity: PublicActivity,
    pub capabilities: Capabilities,
    /// The consumer the calling agent key belongs to; present only on a
    /// channel with routing, so an unrouted response keeps its bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumer: Option<ConsumerSummary>,
}
/// Which of a routed channel's consumers a context response was read for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ConsumerSummary {
    pub id: ConsumerId,
    pub name: ConsumerName,
    /// The consumer's reply label, else its name.
    pub label: String,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PublicActivity {
    pub kind: ActivityKind,
    pub actor: Actor,
    pub conversation: ActivityConversation,
    pub occurred_at: String,
    /// The activity's routing decision; absent on every unrouted event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<ActivityRouting>,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Capabilities {
    /// Exact admitted Telegram forum topic; no creation or enumeration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_lifecycle: Option<cmaeg_core::thread_lifecycle::TopicLifecycleCapabilities>,
    /// Derived from retained native evidence; actual platform permissions are checked per call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_lifecycle: Option<cmaeg_core::thread_lifecycle::ThreadCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_history:
        Option<cmaeg_core::conversation_history::ConversationHistoryCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_media_edits: Option<crate::native_media::Capabilities>,
    /// A fixed 20-message prior window is available on explicit request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_history: Option<bool>,
    /// New exact-message effects; absent until compatible readers are deployed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_effects: Option<super::MessageEffectsCapabilities>,
    /// Approved WhatsApp template replies only; no callbacks or initiated sends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whatsapp_templates: Option<bool>,
    /// Group templates accept header/body components only; absent for other transports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whatsapp_template_components: Option<Vec<String>>,
    pub edit: bool,
    pub typing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forms: Option<bool>,
    pub buttons: String,
    pub cards: String,
    pub attachments: String,
    pub max_text_chars: u32,
    /// What this binding can do with inbound attachments; absent while
    /// inbound media is disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbound_media: Option<InboundMediaCapabilities>,
    /// Explicit native attachments; relay sources require deployment attestation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_media: Option<NativeMediaCapabilities>,
}

#[path = "context_video.rs"]
mod video;
pub use video::NativeVideoCapabilities;
#[path = "context_sticker.rs"]
mod sticker;
pub use sticker::NativeStickerCapabilities;
#[path = "context_media.rs"]
mod media;
#[cfg(test)]
#[path = "context_telegram_media_controls_tests.rs"]
mod telegram_media_controls_tests;
pub use media::NativeMediaCapabilities;
#[path = "context_whatsapp.rs"]
mod whatsapp;
use whatsapp::native_whatsapp_templates;
#[path = "context_capabilities.rs"]
mod capabilities;
/// The event's context as `matched`, the calling consumer, sees it: read
/// through that consumer's own binding and credential.
pub(super) async fn read(
    state: &ToolsState,
    channel: &ResolvedChannel,
    matched: &ConsumerId,
    event: &EventRefPayload,
) -> Result<ContextResponse, Problem> {
    let consumer = consumer_view(channel, matched)?;
    let (activity, activity_verified) = read_activity(state, channel, &consumer, event).await?;
    let inbound_media = inbound_media(state, &consumer, &activity.source);
    Ok(project(
        channel,
        &consumer,
        event,
        activity,
        activity_verified,
        inbound_media,
        state,
    ))
}

/// The consumer a tool call is served as: `matched`, the consumer the caller's
/// key belongs to. A consumer removed after its reference was minted has no
/// view; its event's work belongs to a consumer that no longer exists, so the
/// reference is answered as expired.
pub(super) fn consumer_view<'a>(
    channel: &'a ResolvedChannel,
    matched: &ConsumerId,
) -> Result<ConsumerView<'a>, Problem> {
    channel.consumer(Some(matched)).ok_or_else(|| {
        tracing::warn!(
            channel = %crate::telemetry::channel_hash(channel.id.as_str()),
            "event_ref names a removed consumer"
        );
        Problem::new(ProblemCode::EventRefExpired, 410, "consumer_removed")
    })
}

/// The inbound media capabilities of `consumer`'s binding for an activity
/// from `source`, or `None` while media is disabled, so that the context
/// output is unchanged with the flag off.
pub(super) fn inbound_media(
    state: &ToolsState,
    consumer: &ConsumerView<'_>,
    source: &SourceKind,
) -> Option<InboundMediaCapabilities> {
    let media = state.media.as_ref()?;
    let capabilities = media.capabilities();
    let visuals = match (capabilities.visuals, source) {
        (VisualKind::Keyframes, _) => VisualSupport::Keyframes,
        (VisualKind::Thumbnail, SourceKind::Telegram { .. } | SourceKind::NativeLark(_)) => {
            VisualSupport::PlatformThumbnail
        }
        (VisualKind::Thumbnail, _) => VisualSupport::None,
    };
    let audience = media.audience(consumer);
    Some(InboundMediaCapabilities {
        read_tool: audience.tools,
        transcripts: capabilities.transcriber_enabled,
        visuals,
        downloads: audience.downloads,
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
    consumer: &ConsumerView<'_>,
    event: &EventRefPayload,
) -> Result<(Activity, bool), Problem> {
    let credential = consumer.credential.ok_or_else(unavailable)?;
    let snapshot = state
        .providers
        .event_context(
            &consumer.provider.slug,
            &consumer.provider.binding_id,
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
    consumer: &ConsumerView<'_>,
    event: &EventRefPayload,
    activity: Activity,
    activity_verified: bool,
    inbound_media: Option<InboundMediaCapabilities>,
    state: &ToolsState,
) -> ContextResponse {
    let mut capabilities = Capabilities {
        native_media_edits: crate::native_media::Capabilities::context(
            state, channel, event, &activity,
        ),
        thread_history: (state.discord_thread_history.is_some()
            && super::thread_history::eligible(channel, event, &activity))
        .then_some(true),
        message_effects: (state.telegram_effects_issuance_enabled
            || state.discord_effects_issuance_enabled)
            .then(|| {
                super::MessageEffectsCapabilities::new(
                    activity_verified
                        && ((state.telegram_effects_issuance_enabled
                            && state.telegram_effects.is_some()
                            && super::message_effects::authority::event_supported(
                                channel, event, &activity,
                            ))
                            || (state.discord_effects_issuance_enabled
                                && state.discord_effects.is_some()
                                && super::message_effects::discord::event_supported(
                                    channel, event, &activity,
                                ))),
                    None,
                )
            }),
        whatsapp_templates: ((state.whatsapp_templates_enabled
            && matches!(
                event.reply,
                cmaeg_core::seal::ReplyAddress::NyxidRelay {
                    platform: Platform::Whatsapp,
                    ..
                }
            ))
            || native_whatsapp_templates(
                state.native_whatsapp_templates_enabled
                    && (!matches!(
                        event.reply,
                        cmaeg_core::seal::ReplyAddress::WhatsappGroup(_)
                    ) || state.native_whatsapp_groups_enabled),
                channel,
                event,
                &activity,
                activity_verified,
            ))
        .then_some(true),
        native_media: NativeMediaCapabilities::for_event(
            state,
            channel,
            event,
            &activity,
            activity_verified,
        ),
        inbound_media,
        ..match activity.source {
            SourceKind::NativeLark(_) => Capabilities::native_lark(),
            SourceKind::Whatsapp { .. } => Capabilities::native_whatsapp(),
            SourceKind::Telegram { .. } => Capabilities::native_telegram(),
            SourceKind::Custom => Capabilities::custom(),
            SourceKind::Discord { .. } => Capabilities {
                edit: state.native_discord_edits_enabled,
                ..Capabilities::native_discord()
            },
            SourceKind::NyxidRelay { platform, .. } => {
                Capabilities::relay_with_text_edits(platform, state.relay_text_edits_enabled)
            }
        }
    };
    if activity_verified
        && state.lark_effects.is_some()
        && super::message_effects::lark_authority::event_supported(channel, event, &activity)
    {
        if let SourceKind::NyxidRelay { route_id, .. } = &activity.source {
            if let Some(cmaeg_core::channel::Source::NyxidRelay {
                lark_message_effects: Some(binding),
                ..
            }) = channel.definition.relay_source_for_route(route_id)
            {
                capabilities.message_effects = Some(super::MessageEffectsCapabilities {
                    media_edit: None,
                    delete: binding.delete,
                    reaction: binding.reaction,
                    scope: "confirmed_native_bot_message".into(),
                    max_reactions: u8::from(binding.reaction),
                    expires_at: None,
                });
            }
        }
    }
    super::message_effects::native_lark_context::project(
        state,
        channel,
        event,
        &activity,
        activity_verified,
        &mut capabilities,
    );
    whatsapp::project_forms(
        state,
        channel,
        event,
        &activity,
        activity_verified,
        &mut capabilities,
    );
    let consumer_controls = (activity_verified
        && crate::interactions::native_lark::eligible(channel, event, &activity))
        || event.consumer.is_none()
        || (state.consumer_interactions_enabled
            && activity_verified
            && crate::interactions::owner::eligible(channel, event, &activity));
    crate::interactions::lark::project(
        &mut capabilities,
        &activity,
        event,
        activity_verified,
        state.lark_cards_enabled && consumer_controls,
    );
    let whatsapp_controls =
        if matches!(event.reply, cmaeg_core::seal::ReplyAddress::Whatsapp { .. }) {
            state.native_whatsapp_interactions_enabled
        } else {
            state.whatsapp_interactions_enabled
        };
    if whatsapp_controls
        && consumer_controls
        && activity_verified
        && crate::interactions::whatsapp::eligible(channel, event, &activity)
    {
        capabilities.buttons = "native_interactive".into();
    }
    if state.discord_interactions_enabled
        && consumer_controls
        && activity_verified
        && crate::interactions::discord::eligible(channel, event, &activity)
    {
        capabilities.buttons = "native_components".into();
    }
    // A confirmed successor reference keeps editing after new chains are disabled.
    if activity_verified
        && crate::interactions::discord::eligible(channel, event, &activity)
        && matches!(&activity.kind, ActivityKind::Interaction(result)
            if result.values.get("revision").and_then(serde_json::Value::as_u64).is_some_and(|revision| revision > 1))
    {
        capabilities.edit = true;
    }
    if !consumer_controls && !matches!(activity.source, SourceKind::Custom) {
        capabilities.buttons = "numbered_text".into();
    }
    // A hand-off's actor is the publishing consumer, so no human click could
    // ever match a card prepared from it; its controls are refused, and the
    // projection says so up front so agents can plan around it.
    if event.lineage.is_some() {
        capabilities.buttons = "none".into();
        capabilities.cards = "none".into();
    }
    whatsapp::group_capabilities(&mut capabilities, event);
    ContextResponse {
        received_message: None,
        thread_history: None,
        event_id: event.event_id.clone(),
        channel_id: channel.id.clone(),
        provider_binding_id: consumer.provider.binding_id.clone(),
        source: activity.source.clone(),
        activity_verified,
        expires_at: event.expires_at,
        conversation_id: event.conversation_id.clone(),
        activity: PublicActivity {
            kind: activity.kind,
            actor: activity.actor,
            conversation: activity.conversation,
            occurred_at: activity.occurred_at,
            routing: activity.routing,
        },
        capabilities,
        consumer: summary(channel, consumer),
    }
}
/// The calling consumer as a routed channel presents it; `None` without
/// routing, where the default is the only consumer.
fn summary(channel: &ResolvedChannel, consumer: &ConsumerView<'_>) -> Option<ConsumerSummary> {
    let routing = channel.definition.routing.as_ref()?;
    let name = if consumer.id.is_default() {
        &routing.default_consumer.name
    } else {
        &routing
            .consumers
            .iter()
            .find(|stored| &stored.id == consumer.id)?
            .name
    };
    Some(ConsumerSummary {
        id: consumer.id.clone(),
        name: name.clone(),
        label: consumer.label.to_owned(),
    })
}
pub(super) fn unavailable() -> Problem {
    Problem::new(
        ProblemCode::ProviderUnavailable,
        503,
        "event_context_unavailable",
    )
}
#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
