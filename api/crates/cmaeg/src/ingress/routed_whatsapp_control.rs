//! A verified control consumes only its stored card and original dispatch owner.
use super::*;
use crate::ingress::launch::LaunchRequest;
use cmaeg_core::{
    ids::IdempotencyKey,
    seal::{event_ref::activity_digest, EventRef},
};

impl IngressState {
    pub(super) async fn bind_whatsapp_control(
        &self,
        verified: &VerifiedWhatsappMessage,
        request: &mut LaunchRequest,
        now: i64,
    ) -> Result<(), LaunchOutcome> {
        let Content::Control { locator, context } = &verified.message.content else {
            return Err(refusal(400, "native_whatsapp_control_required"));
        };
        let Some(store) = &self.interactions else {
            return Err(refusal(503, "native_whatsapp_control_storage_unavailable"));
        };
        let route = crate::interactions::whatsapp_authority::route(
            &request.activity.source,
            &verified.message.sender,
        )
        .map_err(|_| refusal(400, "native_whatsapp_control_invalid"))?;
        let (interaction, occurred_at, owner) = store
            .consume_for_dispatch(
                &verified.channel,
                &request.activity,
                &route,
                &verified.message.id,
                locator,
                context,
                None,
            )
            .await
            .map_err(|error| match error {
                crate::interactions::Error::Denied => {
                    refusal(403, "native_whatsapp_control_invalid")
                }
                crate::interactions::Error::Unavailable => {
                    refusal(503, "native_whatsapp_control_pending")
                }
            })?;
        let owner = owner.ok_or_else(|| refusal(403, "native_whatsapp_control_owner_missing"))?;
        request.activity.kind = ActivityKind::Interaction(interaction);
        request.activity.occurred_at = occurred_at;
        request.activity.routing = owner.routing;
        request.consumer = owner.consumer.clone();
        request.conversation_id = owner.conversation_id.clone();
        request.idempotency_key = match &owner.consumer {
            Some(consumer) => IdempotencyKey::for_consumer(&request.activity.event_id, consumer),
            None => IdempotencyKey::from_event(&request.activity.event_id),
        };
        let mut event = request
            .event_ref
            .open(&self.master_keys, now)
            .map_err(|_| refusal(400, "native_whatsapp_control_context_invalid"))?
            .payload;
        event.consumer = owner.consumer;
        event.conversation_id = owner.conversation_id;
        event.partition_key = owner.partition_key;
        event.activity_sha256 = Some(activity_digest(&request.activity));
        request.event_ref = EventRef::seal(&event, &self.master_keys)
            .map_err(|_| refusal(503, "native_whatsapp_control_context_unavailable"))?;
        Ok(())
    }
}
