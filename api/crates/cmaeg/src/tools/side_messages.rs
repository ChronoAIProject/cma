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
    turns::routed::SideMessageAuthority,
};

const TOOL: &str = "sendSideMessage";

impl ToolsState {
    pub(crate) async fn side_message_available(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
    ) -> Result<bool, Problem> {
        let authority = self.side_message_authority();
        let (channel, event) = self
            .side_message_context(caller, reference, &authority)
            .await?;
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
        let authority = self.side_message_authority();
        let (channel, original_event) = self
            .side_message_context(caller, reference, &authority)
            .await?;
        let mut event = original_event.clone();
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
        let gate = SideMessageGate::new(
            self.resolver.clone(),
            channel.clone(),
            original_event,
            caller,
            authority,
        );
        let receipt = delivery.send(&channel, &event, &message, &gate).await;
        store.finish(claim, &receipt).await?;
        Ok(receipt)
    }

    fn side_message_authority(&self) -> SideMessageAuthority {
        SideMessageAuthority::new(
            self.resolver.record_client(),
            self.keys.clone(),
            Arc::new(self.resolver.clone()),
            self.routed_replies
                .as_ref()
                .is_some_and(|routed| routed.requires_retained_binding()),
        )
    }

    async fn side_message_context(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        authority: &SideMessageAuthority,
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
        authority
            .check(caller, &event, current.version)
            .await
            .map_err(super::routed_reply_error)?;
        if time::OffsetDateTime::now_utc().unix_timestamp() >= event.expires_at {
            return Err(super::routed_reply_error(
                crate::turns::routed::ExecutionError::Expired,
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

pub(crate) struct SideMessageGate {
    resolver: ChannelResolver,
    channel: Arc<ResolvedChannel>,
    event: EventRefPayload,
    caller: AgentCaller,
    authority: SideMessageAuthority,
    deadline: tokio::time::Instant,
}

impl SideMessageGate {
    pub(crate) fn new(
        resolver: ChannelResolver,
        channel: Arc<ResolvedChannel>,
        event: EventRefPayload,
        caller: AgentCaller,
        authority: SideMessageAuthority,
    ) -> Self {
        Self {
            resolver,
            channel,
            event,
            caller,
            authority,
            deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(20),
        }
    }
}

impl SendGate for SideMessageGate {
    fn enter_route<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<RouteTicket, GateRefusal>> {
        Box::pin(async { Ok(RouteTicket::empty()) })
    }

    fn commit(&self) -> bool {
        tokio::time::Instant::now() < self.deadline
            && time::OffsetDateTime::now_utc().unix_timestamp() < self.event.expires_at
    }

    fn attempt(&self) -> BoxFuture<'_, Result<AttemptPermit, GateRefusal>> {
        Box::pin(async {
            let refuse = GateRefusal {
                reason: "side_message_authority_changed",
                retry_after: None,
            };
            let verify = async {
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
                self.authority
                    .check(self.caller, &self.event, self.channel.version)
                    .await
                    .map_err(|_| refuse)?;
                if !self.commit() {
                    return Err(refuse);
                }
                Ok(AttemptPermit::empty())
            };
            tokio::time::timeout_at(self.deadline, verify)
                .await
                .unwrap_or(Err(refuse))
        })
    }

    fn progress(&self) {}

    fn transport_deadline(&self) -> Option<tokio::time::Instant> {
        Some(self.deadline)
    }
}
