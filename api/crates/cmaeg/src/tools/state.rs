use super::{context::ContextResponse, MediaDownloads};
use crate::{
    auth::AgentCaller,
    channels::resolver::{ChannelResolver, ResolvedChannel},
    delivery::{
        intents::{DeliveryCommand, DeliveryOutcome, IntentReceipt, ReplySource, TypingReceipt},
        router::DeliverySink,
    },
    media::service::MediaService,
    providers::ProviderClient,
    turns::TurnTracker,
};
use cmaeg_core::{
    ids::{ConsumerId, MessageRef},
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
    seal::{EventRef, EventRefPayload, MasterKeyRing, MessageRefPayload, OpenError},
};
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::Instrument;
mod authority;
#[path = "delivery_timeout.rs"]
mod delivery_timeout;
#[path = "state_validation.rs"]
mod validation;
pub(super) use validation::validate;
#[derive(Clone)]
pub struct ToolsState {
    pub native_typing: Option<Arc<super::NativeTyping>>,
    pub native_whatsapp_forms: Option<Arc<crate::whatsapp::forms::service::Forms>>,
    pub native_media_edits: Option<Arc<crate::native_media::Client>>,
    pub native_lark_effects: Option<Arc<super::NativeLarkEffectsClient>>,
    pub whatsapp_reactions: Option<Arc<super::WhatsappReactionsClient>>,
    pub discord_thread_history: Option<Arc<super::DiscordThreadHistory>>,
    pub telegram_topics: Option<Arc<super::TelegramTopics>>,
    pub discord_threads: Option<Arc<super::DiscordThreads>>,
    pub native_lark_conversation_history: Option<Arc<super::NativeLarkConversationHistory>>,
    pub discord_conversation_history: Option<Arc<super::DiscordConversationHistory>>,
    /// Match the delivery engine's complete ordinary relay mode and wait budget.
    pub complete_replies_enabled: bool,
    pub commonmark_relay: crate::config::CommonmarkRelay,
    pub telegram_effects_issuance_enabled: bool,
    pub native_telegram_media_controls_enabled: bool,
    pub native_telegram_media_control_edits_enabled: bool,
    pub telegram_effects: Option<Arc<super::MessageEffectsClient>>,
    /// Advertise new Lark effects; retained references remain executable when false.
    pub lark_effects_issuance_enabled: bool,
    pub lark_effects: Option<Arc<super::LarkEffectsClient>>,
    /// Advertise templates only after the WhatsApp relay contract is verified.
    pub whatsapp_templates_enabled: bool,
    pub discord_interactions_enabled: bool,
    pub native_discord_media_controls_enabled: bool,
    pub native_discord_media_card_edits_enabled: bool,
    pub native_discord_media_card_replacements_enabled: bool,
    pub native_discord_media_card_sets_enabled: bool,
    pub whatsapp_interactions_enabled: bool,
    /// Native issuance only; retained callback consumption is independent.
    pub native_whatsapp_interactions_enabled: bool,
    pub native_whatsapp_media_controls_enabled: bool,
    pub consumer_interactions_enabled: bool,
    /// Advertise native relay media only after the deployed NyxID contract is verified.
    pub native_media_enabled: bool,
    /// Bounded native Lark image/document/audio/video uploads, independently default-off.
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
    pub native_lark_thread_start_enabled: bool,
    /// Bounded native WhatsApp image/document/audio/sticker/video uploads, independently gated.
    pub native_whatsapp_outbound_media_enabled: bool,
    /// Approved native template replies, independently default-off from relay templates/uploads.
    pub native_whatsapp_templates_enabled: bool,
    pub native_whatsapp_groups_enabled: bool,
    /// Issue and advertise ordinary relay text edits after all readers are upgraded.
    pub relay_text_edits_enabled: bool,
    /// Advertise native Discord text/card editing after the selected reader rollout.
    pub native_discord_edits_enabled: bool,
    pub discord_effects_issuance_enabled: bool,
    pub discord_effects: Option<Arc<super::DiscordEffectsClient>>,
    /// Native Lark/Feishu card issuance after compatible readers are deployed.
    pub lark_cards_enabled: bool,
    pub resolver: ChannelResolver,
    pub keys: Arc<MasterKeyRing>,
    pub providers: Arc<ProviderClient>,
    pub delivery: Arc<dyn DeliverySink>,
    /// Wire together with routed execution; None preserves legacy live tools.
    pub routed_replies: Option<Arc<crate::turns::routed::RoutedToolReplies>>,
    pub tracker: Arc<TurnTracker>,
    pub public_origin: String,
    pub public_api_url: String,
    pub public_api_aliases: Vec<String>,
    pub advertise_public_api_url: bool,
    /// The media service turns share, or `None` while inbound media is
    /// disabled; attachment reads are then refused.
    pub media: Option<Arc<MediaService>>,
    /// The pod's download redemptions and stream slots.
    pub downloads: Arc<MediaDownloads>,
}
impl ToolsState {
    pub fn advertised_api_url(&self) -> &str {
        if self.advertise_public_api_url {
            &self.public_api_url
        } else {
            &self.public_origin
        }
    }

    pub async fn reply(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        message: OutboundMessage,
    ) -> Result<IntentReceipt, Problem> {
        self.reply_with_idempotency(caller, reference, message, None)
            .await
    }
    pub async fn reply_with_idempotency(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        message: OutboundMessage,
        idempotency_key: Option<&str>,
    ) -> Result<IntentReceipt, Problem> {
        super::validate_reply_key(idempotency_key)?;
        let mut observation = crate::telemetry::operations::Operation::new(
            crate::telemetry::names::TOOL_DURATION,
            crate::telemetry::names::TOOL,
            "replyToEvent",
        );
        let result = async {
            validate(&message)?;
            if message.structured_content.is_some() && idempotency_key.is_none() {
                return Err(invalid("structured_content_requires_idempotency_key"));
            }
            if message.native_media_update.is_some() || message.clear_native_attachments {
                return Err(Problem::new(
                    ProblemCode::InvalidRequest,
                    400,
                    if message.clear_native_attachments {
                        "native_attachment_clear_requires_updateMessage"
                    } else {
                        "native_media_update_requires_updateMessage"
                    },
                ));
            }
            let (channel, event, _) = self
                .authorize_event(caller, reference, "replyToEvent")
                .await?;
            let expires_at = event.expires_at;
            if matches!(event.reply, cmaeg_core::seal::ReplyAddress::Whatsapp { .. })
                && crate::whatsapp::forms::compile::requested(&message)
            {
                return Err(Problem::new(
                    ProblemCode::Conflict,
                    409,
                    "whatsapp_form_preparation_required",
                ));
            }
            if let Some(routed) = &self.routed_replies {
                if let Some(receipt) = routed
                    .reply_with_idempotency(
                        caller,
                        &channel,
                        &event,
                        message.clone(),
                        false,
                        idempotency_key,
                    )
                    .await
                    .map_err(super::routed_reply_error)?
                {
                    return Ok(receipt);
                }
            }
            if message.structured_content.is_some() {
                return Err(invalid("structured_content_requires_retained_routing"));
            }
            if message.reply_with_received_sticker {
                return Err(invalid("received_sticker_requires_retained_routing"));
            }
            let (receipt, received) = oneshot::channel();
            let result = self
                .deliver(
                    DeliveryCommand::Reply {
                        trace: tracing::Span::current(),
                        channel,
                        event,
                        message,
                        source: ReplySource::Tool,
                        receipt,
                    },
                    received,
                )
                .await?;
            if matches!(result.delivery, DeliveryOutcome::Confirmed { .. }) {
                if let Err(reason) =
                    self.tracker
                        .record_tool_reply(reference.as_str(), expires_at, now())
                {
                    tracing::warn!(
                        reason,
                        "confirmed tool reply could not retain its dedupe marker"
                    );
                }
            }
            Ok(result)
        }
        .await;
        observation.finish(if result.is_ok() { "success" } else { "error" });
        result
    }
    pub async fn typing(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
    ) -> Result<TypingReceipt, Problem> {
        let mut observation = crate::telemetry::operations::Operation::new(
            crate::telemetry::names::TOOL_DURATION,
            crate::telemetry::names::TOOL,
            "sendTyping",
        );
        let result = async {
            let (channel, event, _) = self
                .authorize_event(caller, reference, "sendTyping")
                .await?;
            if matches!(
                event.reply,
                cmaeg_core::seal::ReplyAddress::Whatsapp { .. }
                    | cmaeg_core::seal::ReplyAddress::Discord { .. }
            ) {
                return self.send_native_typing(caller, &channel, &event).await;
            }
            let (receipt, received) = oneshot::channel();
            self.deliver(
                DeliveryCommand::Typing {
                    trace: tracing::Span::current(),
                    channel,
                    event,
                    receipt,
                },
                received,
            )
            .await
        }
        .await;
        observation.finish(if result.is_ok() { "success" } else { "error" });
        result
    }
    pub async fn context(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
    ) -> Result<ContextResponse, Problem> {
        let mut observation = crate::telemetry::operations::Operation::new(
            crate::telemetry::names::TOOL_DURATION,
            crate::telemetry::names::TOOL,
            "readEventContext",
        );
        let result = async {
            let (channel, event, matched) = self
                .authorize_event(caller, reference, "readEventContext")
                .await?;
            let consumer = super::context::consumer_view(&channel, &matched)?;
            let (activity, verified) =
                super::context::read_activity(self, &channel, &consumer, &event).await?;
            let received_message = self
                .received_message_context(caller, &event, &activity, verified)
                .await;
            let inbound_media = super::context::inbound_media(self, &consumer, &activity.source);
            let mut response = super::context::project(
                &channel,
                &consumer,
                &event,
                activity,
                verified,
                inbound_media,
                self,
            );
            response.received_message = received_message;
            response.capabilities.typing |= self
                .native_typing_capability(caller, &channel, &event, verified)
                .await;
            self.enrich_conversation_capabilities(
                caller,
                &event,
                &channel,
                &mut response.capabilities,
            )
            .await;
            Ok(response)
        }
        .await;
        observation.finish(if result.is_ok() { "success" } else { "error" });
        result
    }
    /// Verify ingress cryptographically without a provider command, cache or read.
    pub async fn verify_context(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        activity: cmaeg_core::activity::Activity,
    ) -> Result<ContextResponse, Problem> {
        activity
            .validate()
            .map_err(|_| invalid("invalid activity"))?;
        let (channel, event, matched) = self
            .authorize_event(caller, reference, "verifyEventContext")
            .await?;
        if !event.verifies_activity(&activity) {
            return Err(invalid("activity does not match the sealed event"));
        }
        let consumer = super::context::consumer_view(&channel, &matched)?;
        let inbound_media = super::context::inbound_media(self, &consumer, &activity.source);
        let received_message = self
            .received_message_context(caller, &event, &activity, true)
            .await;
        let mut response = super::context::project(
            &channel,
            &consumer,
            &event,
            activity,
            true,
            inbound_media,
            self,
        );
        response.received_message = received_message;
        response.capabilities.typing |= self
            .native_typing_capability(caller, &channel, &event, true)
            .await;
        self.enrich_conversation_capabilities(caller, &event, &channel, &mut response.capabilities)
            .await;
        Ok(response)
    }
    pub(super) async fn deliver<T>(
        &self,
        command: DeliveryCommand,
        received: oneshot::Receiver<T>,
    ) -> Result<T, Problem> {
        let timeout = delivery_timeout::for_command(
            &command,
            self.complete_replies_enabled,
            self.commonmark_relay,
        );
        let (channel, event_id) = match &command {
            DeliveryCommand::DurableReply(reply) => {
                (reply.channel.id.as_str(), reply.event.event_id.as_str())
            }
            DeliveryCommand::Reply { channel, event, .. }
            | DeliveryCommand::Typing { channel, event, .. } => {
                (channel.id.as_str(), event.event_id.as_str())
            }
            DeliveryCommand::Update {
                channel,
                message_ref,
                ..
            } => (channel.id.as_str(), message_ref.event_id.as_str()),
            DeliveryCommand::StreamOutcome { turn, .. }
            | DeliveryCommand::RetireStream { turn } => {
                (turn.channel.id.as_str(), turn.activity.event_id.as_str())
            }
        };
        let span = tracing::info_span!("cmaeg.deliver", channel = %crate::telemetry::channel_hash(channel), event_id);
        tokio::time::timeout(
            timeout,
            async {
                self.delivery.submit(command).await.map_err(|_| {
                    Problem::new(ProblemCode::Internal, 503, "delivery_unavailable")
                })?;
                received
                    .await
                    .map_err(|_| Problem::new(ProblemCode::Internal, 503, "delivery_unavailable"))
            }
            .instrument(span),
        )
        .await
        .map_err(|_| Problem::new(ProblemCode::Internal, 504, "delivery_timeout"))?
    }
}
/// The consumer `caller` holds the key of, which must be `issued_to`, the
/// consumer a reference was issued to (`None` is the default). A key of
/// another consumer on the same channel is refused as `consumer_mismatch`:
/// each consumer acts only on its own events, messages and targets.
///
/// Refusals log the channel hash, the tool and the outcome class only, never
/// a key, hash or reference.
pub(crate) fn owner(
    caller: AgentCaller,
    channel: &ResolvedChannel,
    issued_to: Option<&ConsumerId>,
    tool: &'static str,
) -> Result<ConsumerId, Problem> {
    let refused = |outcome: &'static str| {
        let channel = crate::telemetry::channel_hash(channel.id.as_str());
        tracing::warn!(%channel, tool, outcome, "tool call refused");
    };
    let matched = caller.consumer(channel).map_err(|error| {
        refused("forbidden");
        error.problem()
    })?;
    let expected = issued_to
        .cloned()
        .unwrap_or_else(ConsumerId::default_consumer);
    if matched != expected {
        refused("consumer_mismatch");
        return Err(Problem::new(
            ProblemCode::Forbidden,
            403,
            "consumer_mismatch",
        ));
    }
    Ok(matched)
}
fn open_error(error: OpenError) -> Problem {
    match error {
        OpenError::Expired => Problem::new(ProblemCode::EventRefExpired, 410, "event_ref_expired"),
        _ => invalid("event_ref_invalid"),
    }
}
fn invalid(detail: &str) -> Problem {
    Problem::new(ProblemCode::EventRefInvalid, 400, detail)
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
