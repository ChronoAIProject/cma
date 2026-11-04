//! Shared update dispatch for legacy messages and explicit native media patches.
use super::{state::validate, ToolsState};
use crate::{
    auth::AgentCaller,
    delivery::intents::{DeliveryCommand, IntentReceipt},
};
use cmaeg_core::{
    ids::MessageRef,
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
};
use tokio::sync::oneshot;
impl ToolsState {
    pub async fn update(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        message: OutboundMessage,
    ) -> Result<IntentReceipt, Problem> {
        self.update_with_idempotency(caller, reference, message, None)
            .await
    }
    pub async fn update_with_idempotency(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        message: OutboundMessage,
        idempotency_key: Option<&str>,
    ) -> Result<IntentReceipt, Problem> {
        super::validate_reply_key(idempotency_key)?;
        let mut observation = crate::telemetry::operations::Operation::new(
            crate::telemetry::names::TOOL_DURATION,
            crate::telemetry::names::TOOL,
            "updateMessage",
        );
        let result = async {
            validate(&message)?;
            if message.structured_content.is_some() {
                return Err(Problem::new(
                    ProblemCode::InvalidRequest,
                    400,
                    "structured_content_requires_replyToEvent",
                ));
            }
            if message.reply_with_received_sticker {
                return Err(Problem::new(
                    ProblemCode::InvalidRequest,
                    400,
                    "received_sticker_requires_replyToEvent",
                ));
            }
            if message
                .native_attachments
                .iter()
                .any(|a| crate::media::presentations::special(a.kind))
            {
                return Err(Problem::new(
                    ProblemCode::InvalidRequest,
                    422,
                    "native_media_presentation_update_unsupported",
                ));
            }
            let (channel, message_ref, _) = self
                .authorize_message(caller, reference, "updateMessage")
                .await?;
            if message.clear_native_attachments
                && !(message_ref.interaction_record_id.is_some()
                    && message_ref.route_id.starts_with("discord:"))
            {
                return Err(Problem::new(
                    ProblemCode::InvalidRequest,
                    400,
                    "native_attachment_clear_unsupported",
                ));
            }
            if let Some(patch) = &message.native_media_update {
                return self
                    .native_media_update(caller, reference, patch, idempotency_key)
                    .await;
            }
            if message_ref.native_media_edit.is_some() {
                return Err(Problem::new(
                    ProblemCode::InvalidRequest,
                    400,
                    "native_media_update_patch_required",
                ));
            }
            if message_ref.native_lark_effect.is_some() {
                return self.native_lark_update(caller, reference, &message).await;
            }
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
}
