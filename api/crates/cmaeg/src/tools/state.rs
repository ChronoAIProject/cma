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
use std::{sync::Arc, time::Duration};
use tokio::sync::oneshot;
use tracing::Instrument;
#[derive(Clone)]
pub struct ToolsState {
    pub whatsapp_reactions: Option<Arc<super::WhatsappReactionsClient>>,
    pub discord_thread_history: Option<Arc<super::DiscordThreadHistory>>,
    /// Match the delivery engine's complete ordinary relay mode and wait budget.
    pub complete_replies_enabled: bool,
    pub telegram_effects_issuance_enabled: bool,
    pub telegram_effects: Option<Arc<super::MessageEffectsClient>>,
    /// Advertise new Lark effects; retained references remain executable when false.
    pub lark_effects_issuance_enabled: bool,
    pub lark_effects: Option<Arc<super::LarkEffectsClient>>,
    /// Advertise templates only after the WhatsApp relay contract is verified.
    pub whatsapp_templates_enabled: bool,
    pub discord_interactions_enabled: bool,
    pub whatsapp_interactions_enabled: bool,
    /// Native issuance only; retained callback consumption is independent.
    pub native_whatsapp_interactions_enabled: bool,
    pub consumer_interactions_enabled: bool,
    /// Advertise native relay media only after the deployed NyxID contract is verified.
    pub native_media_enabled: bool,
    /// Bounded native WhatsApp image/PDF/audio/sticker uploads, independently default-off.
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

    /// Open `reference` and authorize `caller` as the consumer it was issued
    /// to, returning that consumer with the channel and event. `tool` names
    /// the operation in refusal logs.
    pub async fn authorize_event(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        tool: &'static str,
    ) -> Result<(Arc<ResolvedChannel>, EventRefPayload, ConsumerId), Problem> {
        let event = reference
            .open(&self.keys, now())
            .map_err(open_error)?
            .payload;
        let channel = self
            .resolver
            .resolve(event.channel_id.as_str())
            .await
            .map_err(|error| error.problem())?;
        let matched = owner(caller, &channel, event.consumer.as_ref(), tool)?;
        if event.record_id != channel.pointer.record_id {
            return Err(invalid("event channel does not match its record"));
        }
        Ok((channel, event, matched))
    }
    /// Open `reference` and authorize `caller` as the consumer whose message
    /// it names, as [`ToolsState::authorize_event`] does for events.
    pub async fn authorize_message(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        tool: &'static str,
    ) -> Result<(Arc<ResolvedChannel>, MessageRefPayload, ConsumerId), Problem> {
        let message = reference
            .open(&self.keys)
            .map_err(|_| invalid("message_ref_invalid"))?
            .payload;
        let channel = if message.received_reaction.is_some()
            || message.whatsapp_reaction.is_some()
            || message.lark_effect.is_some()
            || message.discord_message.is_some()
            || (message.interaction_record_id.is_some() && message.route_id.starts_with("discord:"))
        {
            self.resolver
                .resolve_current(message.channel_id.as_str())
                .await
        } else {
            self.resolver.resolve(message.channel_id.as_str()).await
        }
        .map_err(|error| error.problem())?;
        let matched = owner(caller, &channel, message.consumer.as_ref(), tool)?;
        Ok((channel, message, matched))
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
            let (channel, event, _) = self
                .authorize_event(caller, reference, "replyToEvent")
                .await?;
            let expires_at = event.expires_at;
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
    pub async fn update(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        message: OutboundMessage,
    ) -> Result<IntentReceipt, Problem> {
        let mut observation = crate::telemetry::operations::Operation::new(
            crate::telemetry::names::TOOL_DURATION,
            crate::telemetry::names::TOOL,
            "updateMessage",
        );
        let result = async {
            validate(&message)?;
            let (channel, message_ref, _) = self
                .authorize_message(caller, reference, "updateMessage")
                .await?;
            let (receipt, received) = oneshot::channel();
            self.deliver(
                DeliveryCommand::Update {
                    trace: tracing::Span::current(),
                    channel,
                    message_ref,
                    message,
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
            Ok(response)
        }
        .await;
        observation.finish(if result.is_ok() { "success" } else { "error" });
        result
    }
    /// Verify an ingress snapshot before any provider command exists. No local
    /// cache or provider read participates in this cryptographic attestation.
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
        Ok(response)
    }
    pub(super) async fn deliver<T>(
        &self,
        command: DeliveryCommand,
        received: oneshot::Receiver<T>,
    ) -> Result<T, Problem> {
        // These include queueing/pacing before the exchange. Routed tool replies
        // use their separate 20 s window and 30 s claims before reaching this path.
        // Structured relay replies can fall back to complete text after the
        // engine checks their Activity. Their outer wait is also 18 s; native
        // card/control transport still uses the configured ordinary timeout.
        // Template media lives in its components, not native_attachments; both
        // use the same 60 s exchange and need room for queueing/pacing here.
        let timeout = if matches!(&command, DeliveryCommand::Reply { message, .. }
            if !message.native_attachments.is_empty()
                || message.whatsapp_template.as_ref().is_some_and(|template| template.has_media()))
        {
            Duration::from_secs(65)
        } else if self.complete_replies_enabled
            && matches!(&command, DeliveryCommand::Reply { event, message, .. }
                if matches!(event.reply, cmaeg_core::seal::ReplyAddress::NyxidRelay { .. })
                    && message.whatsapp_template.is_none())
        {
            crate::delivery::relay_client::COMPLETE_REPLY_TOOL_TIMEOUT
        } else {
            Duration::from_secs(8)
        };
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
            DeliveryCommand::StreamOutcome { turn, .. } => {
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
pub(super) fn owner(
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
pub(super) fn validate(message: &OutboundMessage) -> Result<(), Problem> {
    message.validate().map_err(|error| {
        Problem::new(
            if error.message.contains("32768") {
                ProblemCode::PayloadTooLarge
            } else {
                ProblemCode::InvalidRequest
            },
            if error.message.contains("32768") {
                413
            } else {
                400
            },
            error.to_string(),
        )
    })
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
