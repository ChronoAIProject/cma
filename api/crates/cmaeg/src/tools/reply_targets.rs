//! Provider-neutral delayed replies to an authenticated event's original destination.
mod fence;
#[cfg(test)]
mod native_fixture;
#[cfg(test)]
mod native_tests;
pub(crate) mod receipts;
use super::{
    context,
    state::{owner, validate},
    ToolsState,
};
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    delivery::intents::{DeliveryCommand, IntentReceipt, ReplySource},
};
use cmaeg_core::{
    channel::{Secret, Source},
    ids::ConsumerId,
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
    seal::{
        EventRef, OpenError, ReplyAddress, ReplyTargetPayload, ReplyTargetRef, REPLY_TARGET_TTL,
    },
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::oneshot;
use utoipa::ToSchema;

/// The tool name reply-target creation is logged under.
const CREATE: &str = "createReplyTarget";
#[derive(Clone, Serialize, ToSchema)]
pub struct ReplyTargetResponse {
    pub reply_target_ref: ReplyTargetRef,
    pub expires_at: i64,
}
impl ToolsState {
    pub async fn create_reply_target(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
    ) -> Result<ReplyTargetResponse, Problem> {
        let (_, mut event, _) = self.authorize_event(caller, reference, CREATE).await?;
        let channel = self
            .resolver
            .resolve_current(event.channel_id.as_str())
            .await
            .map_err(|e| e.problem())?;
        // Recheck against the live definition: a removed consumer or a
        // rotated key no longer authorizes a new target.
        let matched = owner(caller, &channel, event.consumer.as_ref(), CREATE)?;
        destination(&channel, &event.reply)?;
        if let Some(routed) = &self.routed_replies {
            routed
                .authorize_reply_target_mint(&event)
                .await
                .map_err(super::routed_reply_error)?;
        }
        if !context::read(self, &channel, &matched, &event)
            .await?
            .activity_verified
        {
            return Err(invalid("verified event context is required"));
        }
        // The durable capability carries no short-lived NyxID reply credential.
        if let ReplyAddress::NyxidRelay {
            reply_token,
            reply_token_expires_at,
            ..
        } = &mut event.reply
        {
            *reply_token = Secret::new("");
            *reply_token_expires_at = 1;
        }
        let issued_at = now();
        if issued_at >= event.expires_at {
            return Err(Problem::new(
                ProblemCode::EventRefExpired,
                410,
                "event_ref_expired",
            ));
        }
        // The target is sealed for the consumer that created it: its slug,
        // binding and key hash. The event inside still names that consumer,
        // so the payload's shape is unchanged.
        let (Some(consumer), Some(agent_key_hash)) = (
            channel.consumer(Some(&matched)),
            channel.consumer_key_hash(Some(&matched)),
        ) else {
            tracing::error!("matched consumer has no view; reply target refused");
            return Err(unavailable());
        };
        let payload = ReplyTargetPayload {
            v: 1,
            provider: consumer.provider.slug.clone(),
            binding_id: consumer.provider.binding_id.clone(),
            agent_key_hash: *agent_key_hash,
            issued_at,
            expires_at: issued_at + REPLY_TARGET_TTL,
            event,
        };
        if let Some(routed) = &self.routed_replies {
            routed
                .authorize_reply_target_mint(&payload.event)
                .await
                .map_err(super::routed_reply_error)?;
        }
        let reply_target_ref =
            ReplyTargetRef::seal(&payload, &self.keys).map_err(|_| unavailable())?;
        Ok(ReplyTargetResponse {
            reply_target_ref,
            expires_at: payload.expires_at,
        })
    }
    pub async fn reply_to_target(
        &self,
        caller: AgentCaller,
        reference: &ReplyTargetRef,
        request_id: &str,
        message: OutboundMessage,
    ) -> Result<IntentReceipt, Problem> {
        validate(&message)?;
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
        if request_id.is_empty()
            || request_id.len() > 128
            || !request_id.bytes().all(|b| (33..=126).contains(&b))
        {
            return Err(invalid(
                "a 1..128 byte printable Idempotency-Key is required",
            ));
        }
        let target = reference
            .open(&self.keys, now())
            .map_err(|e| match e {
                OpenError::Expired => {
                    Problem::new(ProblemCode::EventRefExpired, 410, "reply_target_expired")
                }
                _ => invalid("reply_target_invalid"),
            })?
            .payload;
        let channel = self.authorize_target(caller, &target).await?;
        if let Some(routed) = &self.routed_replies {
            if let Some(result) = routed
                .reply_delayed(caller, &target, request_id, message.clone())
                .await
                .map_err(super::routed_reply_error)?
            {
                return Ok(result);
            }
        }
        let store = receipts::Store::new(self.resolver.record_client(), self.keys.clone());
        let claim = match store
            .claim(
                &target,
                request_id,
                &message,
                &channel.definition.owner.subject,
            )
            .await?
        {
            receipts::Admission::Replay(receipt) => return Ok(receipt),
            receipts::Admission::Claimed(claim) => claim,
        };
        // Recheck after durable admission; rotation/deletion during the store call must refuse dispatch.
        let channel = self.authorize_target(caller, &target).await?;
        if let Some(routed) = &self.routed_replies {
            if let Some(result) = routed
                .reply(caller, &channel, &target.event, message.clone(), true)
                .await
                .map_err(super::routed_reply_error)?
            {
                store.finish(claim, &result).await?;
                return Ok(result);
            }
        }
        let (receipt, received) = oneshot::channel();
        let result = self
            .deliver(
                DeliveryCommand::Reply {
                    trace: tracing::Span::current(),
                    channel,
                    event: target.event,
                    message,
                    source: ReplySource::Tool,
                    receipt,
                },
                received,
            )
            .await
            .unwrap_or_else(|_| receipts::uncertain("delivery_unconfirmed"));
        store.finish(claim, &result).await?;
        Ok(result)
    }
    async fn authorize_target(
        &self,
        caller: AgentCaller,
        target: &ReplyTargetPayload,
    ) -> Result<Arc<ResolvedChannel>, Problem> {
        if now() >= target.expires_at {
            return Err(Problem::new(
                ProblemCode::EventRefExpired,
                410,
                "reply_target_expired",
            ));
        }
        let channel = self
            .resolver
            .resolve_current(target.event.channel_id.as_str())
            .await
            .map_err(|e| e.problem())?;
        validate_current_target(caller, target, &channel, now())?;
        Ok(channel)
    }
}
pub(crate) fn validate_current_target(
    caller: AgentCaller,
    target: &ReplyTargetPayload,
    channel: &ResolvedChannel,
    now: i64,
) -> Result<(), Problem> {
    if now >= target.expires_at {
        return Err(Problem::new(
            ProblemCode::EventRefExpired,
            410,
            "reply_target_expired",
        ));
    }
    caller.consumer(channel).map_err(|e| e.problem())?;
    // The target belongs to the consumer whose binding it names, and only
    // that consumer's current key may use it: removing the consumer,
    // rotating its key or rebinding it revokes the target.
    let default = ConsumerId::default_consumer();
    let issued_to = target.event.consumer.as_ref().unwrap_or(&default);
    let owner_matches = channel
        .definition
        .consumers()
        .find(|consumer| consumer.provider.binding_id == target.binding_id)
        .is_some_and(|consumer| {
            consumer.provider.slug == target.provider
                && consumer.id == issued_to
                && channel
                    .consumer_key_hash(Some(consumer.id))
                    .is_some_and(|current| caller.matches(current))
        });
    if !caller.matches(&target.agent_key_hash)
        || channel.pointer.record_id != target.event.record_id
        || !owner_matches
    {
        let channel = crate::telemetry::channel_hash(channel.id.as_str());
        tracing::warn!(
            %channel,
            tool = "replyToTarget",
            outcome = "reply_target_revoked",
            "tool call refused"
        );
        return Err(Problem::new(
            ProblemCode::Forbidden,
            403,
            "reply_target_revoked",
        ));
    }
    destination(channel, &target.event.reply)?;
    Ok(())
}

pub(super) fn destination(channel: &ResolvedChannel, reply: &ReplyAddress) -> Result<(), Problem> {
    let valid = match reply {
        ReplyAddress::Whatsapp { .. } | ReplyAddress::WhatsappGroup(_) => crate::whatsapp::credentials(&channel.definition, reply).is_some(),
        ReplyAddress::Discord { bot_id, .. } => crate::discord::rest::token(channel, bot_id).is_some(),
        ReplyAddress::NyxidRelay { route_id, platform, .. } => channel.definition.secrets.channel_key.is_some()
            && channel.definition.sources.iter().any(|s| matches!(s, Source::NyxidRelay { route_ids, platform: current, .. } if current==platform && route_ids.contains(route_id))),
        ReplyAddress::Telegram { bot_id, .. } => channel.definition.secrets.telegram_bot_token.is_some()
            && channel.definition.sources.iter().any(|s| matches!(s, Source::Telegram { bot_id:current, .. } if current==bot_id)),
        _ => return Err(invalid("delayed replies require a chat destination")),
    };
    if !valid {
        return Err(Problem::new(
            ProblemCode::Forbidden,
            403,
            "reply_destination_revoked",
        ));
    }

    Ok(())
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
fn invalid(detail: &str) -> Problem {
    Problem::new(ProblemCode::InvalidRequest, 400, detail)
}
fn unavailable() -> Problem {
    Problem::new(ProblemCode::Internal, 503, "reply_target_unavailable")
}
