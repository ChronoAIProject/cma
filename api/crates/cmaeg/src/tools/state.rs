use super::context::ContextResponse;
use crate::{
    auth::AgentCaller,
    channels::resolver::{ChannelResolver, ResolvedChannel},
    delivery::intents::{
        DeliveryCommand, DeliveryOutcome, IntentReceipt, ReplySource, TypingReceipt,
    },
    providers::ProviderClient,
    turns::TurnTracker,
};
use cmaeg_core::{
    ids::MessageRef,
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
    seal::{EventRef, EventRefPayload, MasterKeyRing, MessageRefPayload, OpenError},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot};
use tracing::Instrument;
#[derive(Clone)]
pub struct ToolsState {
    pub resolver: ChannelResolver,
    pub keys: Arc<MasterKeyRing>,
    pub providers: Arc<ProviderClient>,
    pub delivery: mpsc::Sender<DeliveryCommand>,
    pub tracker: Arc<TurnTracker>,
    pub public_origin: String,
}
impl ToolsState {
    pub async fn authorize_event(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
    ) -> Result<(Arc<ResolvedChannel>, EventRefPayload), Problem> {
        let event = reference
            .open(&self.keys, now())
            .map_err(open_error)?
            .payload;
        let channel = self
            .resolver
            .resolve(event.channel_id.as_str())
            .await
            .map_err(|error| error.problem())?;
        caller
            .authorize(&channel)
            .map_err(|error| error.problem())?;
        if event.record_id != channel.pointer.record_id {
            return Err(invalid("event channel does not match its record"));
        }
        Ok((channel, event))
    }
    pub async fn authorize_message(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
    ) -> Result<(Arc<ResolvedChannel>, MessageRefPayload), Problem> {
        let message = reference
            .open(&self.keys)
            .map_err(|_| invalid("message_ref_invalid"))?
            .payload;
        let channel = self
            .resolver
            .resolve(message.channel_id.as_str())
            .await
            .map_err(|error| error.problem())?;
        caller
            .authorize(&channel)
            .map_err(|error| error.problem())?;
        Ok((channel, message))
    }
    pub async fn reply(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        message: OutboundMessage,
    ) -> Result<IntentReceipt, Problem> {
        let mut observation = crate::telemetry::operations::Operation::new(
            crate::telemetry::names::TOOL_DURATION,
            crate::telemetry::names::TOOL,
            "replyToEvent",
        );
        let result = async {
            validate(&message)?;
            let (channel, event) = self.authorize_event(caller, reference).await?;
            let expires_at = event.expires_at;
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
            let (channel, message_ref) = self.authorize_message(caller, reference).await?;
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
            let (channel, event) = self.authorize_event(caller, reference).await?;
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
            let (channel, event) = self.authorize_event(caller, reference).await?;
            super::context::read(self, &channel, &event).await
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
        let (channel, event) = self.authorize_event(caller, reference).await?;
        if !event.verifies_activity(&activity) {
            return Err(invalid("activity does not match the sealed event"));
        }
        Ok(super::context::project(&channel, &event, activity, true))
    }
    pub(super) async fn deliver<T>(
        &self,
        command: DeliveryCommand,
        received: oneshot::Receiver<T>,
    ) -> Result<T, Problem> {
        let (channel, event_id) = match &command {
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
            Duration::from_secs(8),
            async {
                self.delivery.send(command).await.map_err(|_| {
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
