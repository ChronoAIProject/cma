//! A side message owns a durable receipt independent of the event's answer.
use std::sync::Arc;

use cmaeg_core::{
    channel::Secret,
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
    seal::{EventRef, EventRefPayload, ReplyAddress, REPLY_TARGET_TTL},
};
use futures_util::future::BoxFuture;

use super::{
    reply_targets::{destination, receipts},
    state::owner,
    ToolsState,
};
use crate::{
    auth::AgentCaller,
    channels::resolver::{ChannelResolver, ResolvedChannel},
    delivery::{
        gate::{AttemptPermit, GateRefusal, RouteTicket, SendGate},
        intents::IntentReceipt,
        side_messages::SideMessageDelivery,
    },
};

const TOOL: &str = "sendSideMessage";

impl ToolsState {
    pub(crate) async fn side_message_available(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
    ) -> Result<bool, Problem> {
        let (channel, event) = self.side_message_context(caller, reference).await?;
        Ok(side_destination(&channel, &event.reply).is_ok())
    }

    pub(crate) async fn send_side_message(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        request_id: &str,
        delivery: &SideMessageDelivery,
        text: String,
    ) -> Result<IntentReceipt, Problem> {
        super::validate_reply_key(Some(request_id))?;
        let (channel, mut event) = self.side_message_context(caller, reference).await?;
        side_destination(&channel, &event.reply)?;
        if text.trim().is_empty() {
            return Err(Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "side_message_invalid",
            ));
        }
        let message = OutboundMessage {
            structured_content: None,
            reply_with_received_sticker: false,
            clear_native_attachments: false,
            native_media_update: None,
            whatsapp_template: None,
            native_attachments: vec![],
            thread_reply: matches!(
                event.reply,
                ReplyAddress::NyxidRelay {
                    thread_reply: true,
                    ..
                }
            ),
            text: Some(text),
            blocks: vec![],
            reply_to: None,
        };
        message
            .validate()
            .map_err(|_| Problem::new(ProblemCode::InvalidRequest, 400, "side_message_invalid"))?;
        let send_reply = &mut event.reply;
        // Side messages leave the single-use reply token with the answer lane.
        if let ReplyAddress::NyxidRelay {
            reply_token,
            reply_token_expires_at,
            ..
        } = send_reply
        {
            *reply_token = Secret::new("");
            *reply_token_expires_at = 1;
        }

        let store = receipts::Store::new(self.resolver.record_client(), self.keys.clone());
        let claim = match claim_side_message(
            &store,
            &event,
            request_id,
            &message,
            &channel.definition.owner.subject,
        )
        .await?
        {
            receipts::Admission::Claimed(claim) => claim,
            receipts::Admission::Replay(receipt) => return Ok(receipt),
        };
        let gate = SideMessageGate {
            resolver: self.resolver.clone(),
            channel: channel.clone(),
            event: event.clone(),
            caller,
        };
        let receipt = delivery.send(&channel, &event, &message, &gate).await;
        store.finish(claim, &receipt).await?;
        Ok(receipt)
    }

    async fn side_message_context(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
    ) -> Result<(Arc<ResolvedChannel>, EventRefPayload), Problem> {
        let (_, event, _) = self.authorize_event(caller, reference, TOOL).await?;
        let current = self
            .resolver
            .resolve_current(event.channel_id.as_str())
            .await
            .map_err(|error| error.problem())?;
        owner(caller, &current, event.consumer.as_ref(), TOOL)?;
        if current.pointer.record_id != event.record_id {
            return Err(Problem::new(
                ProblemCode::Forbidden,
                403,
                "side_message_revoked",
            ));
        }
        Ok((current, event))
    }
}

pub(crate) async fn claim_side_message(
    store: &receipts::Store,
    event: &EventRefPayload,
    request_id: &str,
    message: &OutboundMessage,
    owner: &str,
) -> Result<receipts::Admission, Problem> {
    let scope = receipts::hash(&(
        &event.channel_id,
        &event.record_id,
        &event.event_id,
        &event.conversation_id,
        &event.consumer,
        &event.reply,
    ));
    store
        .claim_scoped(
            "event_side_message_v1",
            "cmaeg.event-side-message.v1",
            scope,
            request_id,
            receipts::hash(message),
            owner,
            event.expires_at.saturating_add(REPLY_TARGET_TTL),
        )
        .await
}

fn side_destination(channel: &ResolvedChannel, reply: &ReplyAddress) -> Result<(), Problem> {
    if matches!(reply, ReplyAddress::NativeLark(_)) {
        return crate::native_lark::credentials(&channel.definition, reply)
            .map(|_| ())
            .ok_or_else(|| {
                Problem::new(
                    ProblemCode::Forbidden,
                    403,
                    "side_message_destination_revoked",
                )
            });
    }
    destination(channel, reply)
}

struct SideMessageGate {
    resolver: ChannelResolver,
    channel: Arc<ResolvedChannel>,
    event: EventRefPayload,
    caller: AgentCaller,
}

impl SendGate for SideMessageGate {
    fn enter_route<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<RouteTicket, GateRefusal>> {
        Box::pin(async { Ok(RouteTicket::empty()) })
    }

    fn commit(&self) -> bool {
        time::OffsetDateTime::now_utc().unix_timestamp() < self.event.expires_at
    }

    fn attempt(&self) -> BoxFuture<'_, Result<AttemptPermit, GateRefusal>> {
        Box::pin(async {
            let refuse = GateRefusal {
                reason: "side_message_authority_changed",
                retry_after: None,
            };
            let current = self
                .resolver
                .resolve_current(self.event.channel_id.as_str())
                .await
                .map_err(|_| refuse)?;
            if !self.commit()
                || current.version != self.channel.version
                || current.pointer.record_id != self.event.record_id
                || owner(self.caller, &current, self.event.consumer.as_ref(), TOOL).is_err()
                || side_destination(&current, &self.event.reply).is_err()
            {
                return Err(refuse);
            }
            Ok(AttemptPermit::empty())
        })
    }

    fn progress(&self) {}
}
