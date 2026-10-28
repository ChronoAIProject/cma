//! One total short deadline includes fresh authorization and every storage access.
use super::{state::owner, ToolsState};
use crate::{
    auth::AgentCaller,
    delivery::intents::{DeliveryOutcome, IntentReceipt},
    whatsapp::forms::{self, service::FormPreparationReceipt},
};
use cmaeg_core::{
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
    seal::{EventRef, EventRefPayload, ReplyAddress},
};
use std::{sync::Arc, time::Duration};

impl ToolsState {
    async fn form_authority(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
    ) -> Result<
        (
            Arc<crate::channels::resolver::ResolvedChannel>,
            EventRefPayload,
        ),
        Problem,
    > {
        let event = reference
            .open(&self.keys, time::OffsetDateTime::now_utc().unix_timestamp())
            .map_err(|e| match e {
                cmaeg_core::seal::OpenError::Expired => {
                    Problem::new(ProblemCode::EventRefExpired, 410, "event_ref_expired")
                }
                _ => Problem::new(ProblemCode::EventRefInvalid, 400, "event_ref_invalid"),
            })?
            .payload;
        let channel = self
            .resolver
            .resolve_current(event.channel_id.as_str())
            .await
            .map_err(|e| e.problem())?;
        owner(
            caller,
            &channel,
            event.consumer.as_ref(),
            "prepareFormReply",
        )?;
        if event.record_id != channel.pointer.record_id
            || !matches!(event.reply, ReplyAddress::Whatsapp { .. })
        {
            return Err(Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "native_whatsapp_form_required",
            ));
        }
        Ok((channel, event))
    }
    pub async fn prepare_form_reply(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        message: OutboundMessage,
        key: &str,
    ) -> Result<FormPreparationReceipt, Problem> {
        super::validate_reply_key(Some(key))?;
        tokio::time::timeout(Duration::from_secs(2), async {
            let forms = self
                .native_whatsapp_forms
                .as_ref()
                .ok_or_else(unavailable)?;
            let (channel, event) = self.form_authority(caller, reference).await?;
            forms
                .prepare(&channel, &event, &message, key)
                .await
                .map_err(problem)
        })
        .await
        .map_err(|_| {
            Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "whatsapp_form_admission_unconfirmed_retry_same_key",
            )
        })?
    }
    pub async fn read_form_preparation(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
    ) -> Result<FormPreparationReceipt, Problem> {
        tokio::time::timeout(Duration::from_secs(2), async {
            let forms = self
                .native_whatsapp_forms
                .as_ref()
                .ok_or_else(unavailable)?;
            let (channel, event) = self.form_authority(caller, reference).await?;
            forms.status(&channel, &event).await.map_err(problem)
        })
        .await
        .map_err(|_| {
            Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "whatsapp_form_status_unavailable",
            )
        })?
    }
    pub async fn reply_with_preparation(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        message: OutboundMessage,
        key: Option<&str>,
        selector: Option<&str>,
    ) -> Result<IntentReceipt, Problem> {
        let Some(selector) = selector else {
            return self
                .reply_with_idempotency(caller, reference, message, key)
                .await;
        };
        super::validate_reply_key(key)?;
        let key = key.ok_or_else(|| {
            Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "whatsapp_form_commit_key_required",
            )
        })?;
        if uuid::Uuid::parse_str(selector).is_err() || !forms::compile::requested(&message) {
            return Err(Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "whatsapp_form_preparation_ref_invalid",
            ));
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        let result = tokio::time::timeout_at(deadline, async {
            let forms = self
                .native_whatsapp_forms
                .as_ref()
                .ok_or_else(unavailable)?;
            let (channel, event) = self.form_authority(caller, reference).await?;
            let receipt = forms
                .commit(&channel, &event, &message, selector, key, deadline)
                .await
                .map_err(problem)?;
            if matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }) {
                self.tracker
                    .record_tool_reply(
                        reference.as_str(),
                        event.expires_at,
                        time::OffsetDateTime::now_utc().unix_timestamp(),
                    )
                    .map_err(|_| unavailable())?;
            }
            Ok(receipt)
        })
        .await;
        result.map_err(|_| {
            Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "whatsapp_form_commit_unconfirmed_read_status",
            )
        })?
    }
}
fn unavailable() -> Problem {
    Problem::new(
        ProblemCode::ProviderUnavailable,
        503,
        "native_whatsapp_forms_unavailable",
    )
}
fn problem(error: forms::Error) -> Problem {
    match error {
        forms::Error::Invalid => {
            Problem::new(ProblemCode::InvalidRequest, 400, "whatsapp_form_invalid")
        }
        forms::Error::Conflict => Problem::new(
            ProblemCode::Conflict,
            409,
            "whatsapp_form_preparation_conflict",
        ),
        forms::Error::Revoked => Problem::new(
            ProblemCode::Forbidden,
            403,
            "whatsapp_form_authority_revoked",
        ),
        forms::Error::Pending => Problem::new(
            ProblemCode::Conflict,
            409,
            "whatsapp_form_preparation_pending",
        ),
        forms::Error::Missing => Problem::new(
            ProblemCode::InvalidRequest,
            404,
            "whatsapp_form_preparation_not_found",
        ),
        forms::Error::Unavailable => unavailable(),
    }
}

#[cfg(test)]
#[path = "form_preparation_tests.rs"]
mod tests;
