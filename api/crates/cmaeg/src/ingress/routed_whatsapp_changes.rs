//! Admission binds coexistence changes only to a retained incoming message.
use super::{refusal, *};
use crate::ingress::routing_store::AdmittedPlan;
use crate::whatsapp::{message_context, webhook::Content};

impl IngressState {
    pub(super) async fn whatsapp_change_original(
        &self,
        verified: &VerifiedWhatsappMessage,
        target: &str,
        now: i64,
    ) -> Result<AdmittedPlan, LaunchOutcome> {
        if !message_context::opted_in(&verified.source) {
            return Err(refusal(
                403,
                "native_whatsapp_message_events_consent_required",
            ));
        }
        let routing = self
            .routed
            .as_ref()
            .ok_or_else(|| refusal(503, "native_whatsapp_admission_unavailable"))?;
        let event_id = webhook::event_id(&verified.source, target)
            .map_err(|_| refusal(400, "native_whatsapp_target_invalid"))?;
        let identity = EventIdentity {
            owner: verified.channel.definition.owner.subject.clone(),
            channel_id: verified.channel.id.clone(),
            record_id: verified.channel.definition.record_id.clone(),
            event_id,
        };
        let original = match routing.plans.load(&identity).await {
            Ok(Some(plan)) => plan,
            Ok(None)
                if verified
                    .received_at
                    .saturating_sub(verified.message.timestamp)
                    < 120 =>
            {
                return Err(refusal(503, "native_whatsapp_target_pending"));
            }
            Ok(None) => return Err(refusal(422, "native_whatsapp_target_unavailable")),
            Err(_) => return Err(refusal(503, "native_whatsapp_original_unavailable")),
        };
        let same =
            webhook::extract(&verified.source, &original.source_body).is_ok_and(|messages| {
                messages.iter().any(|message| {
                    message.id == target
                        && message.sender == verified.message.sender
                        && message.group_id.is_none()
                        && crate::whatsapp::message_changes::matches_original(
                            &verified.message.content,
                            &message.content,
                        )
                })
            });
        if !same || !matches!(original.activity.kind, ActivityKind::Message { .. }) {
            return Err(refusal(403, "native_whatsapp_target_authority_unavailable"));
        }
        let records = self.resolver.record_client();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            message_context::check(
                &records,
                &self.master_keys,
                &verified.channel,
                &original,
                &verified.message.sender,
                target,
                now,
            ),
        )
        .await;
        match result {
            Ok(Ok(())) => Ok(original),
            Ok(Err(message_context::Error::Missing | message_context::Error::Denied)) => Err(
                refusal(403, "native_whatsapp_original_authority_unavailable"),
            ),
            _ => Err(refusal(503, "native_whatsapp_original_unavailable")),
        }
    }
    pub(super) async fn remember_whatsapp_original(
        &self,
        verified: &VerifiedWhatsappMessage,
        existing: bool,
        now: i64,
    ) -> Result<(), LaunchOutcome> {
        if existing
            || !self.native_whatsapp_message_events_enabled
            || !message_context::opted_in(&verified.source)
            || verified.message.group_id.is_some()
            || verified.message.timestamp <= now - message_context::TTL
            || !matches!(
                verified.message.content,
                Content::Text { .. } | Content::Media { .. }
            )
        {
            return Ok(());
        }
        let context = message_context::Context::capture(verified, now)
            .map_err(|_| refusal(403, "native_whatsapp_original_context_invalid"))?;
        let records = self.resolver.record_client();
        match tokio::time::timeout(
            std::time::Duration::from_millis(250),
            context.remember(
                &records,
                &self.master_keys,
                &verified.channel,
                &verified.source,
            ),
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message_context::Error::Denied)) => {
                Err(refusal(403, "native_whatsapp_original_context_changed"))
            }
            _ => Err(refusal(503, "native_whatsapp_original_context_unavailable")),
        }
    }
}
