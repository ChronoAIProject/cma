//! Shared authenticated tool operations used by REST and MCP.
mod attachment_compose;
mod attachment_view;
mod attachments;
mod context;
mod native_typing;
pub use native_typing::NativeTyping;
mod form_preparation;
pub use crate::whatsapp::forms::service::{
    FormCustomerMessage, FormPreparationReceipt, FormPreparationState,
};
#[cfg(test)]
#[allow(dead_code)]
#[path = "../../tests/fake_cma.rs"]
mod fake_cma;
mod history;
mod thread_history;
pub use history::DiscordConversationHistory;
mod native_lark_history;
pub use native_lark_history::NativeLarkConversationHistory;
mod threads;
pub use thread_history::{DiscordThreadHistory, ThreadHistory, ThreadHistoryMessage};
pub use threads::{DiscordThreads, TelegramTopics};
mod download_access;
mod media_links;
mod media_redemption;
pub(crate) mod message_effects;
pub(crate) mod reply_targets;
pub(crate) mod side_messages;
pub(crate) mod state;
pub use attachment_view::{
    AttachmentImage, AttachmentImageOrigin, AttachmentStatus, AttachmentText, AttachmentTextSource,
    AttachmentUnavailableReason, AttachmentView, AttachmentViewKind, InboundMediaCapabilities,
    MediaAuthorization, MediaDownload, VisualSupport,
};
pub use attachments::{
    valid_request, AttachmentRead, BUDGET_ACQUIRE_MAX, BUDGET_RETRY_MS, MAX_INDEX, MAX_PAGE,
    MAX_RESULT_IMAGE_BYTES, REST_VIEW_WEIGHT, TOOL_WAIT,
};
pub use context::{
    Capabilities, ConsumerSummary, ContextResponse, NativeMediaCapabilities,
    NativeStickerCapabilities, NativeVideoCapabilities, PublicActivity,
};
pub(crate) use download_access::{Authorized, Refusal};
pub(crate) use media_links::safe_name;
pub use media_links::{
    LinkRefusal, MediaDownloads, DOWNLOAD_RESPONSE_WEIGHT, DOWNLOAD_STREAMS_GLOBAL,
    DOWNLOAD_STREAMS_PER_CHANNEL,
};
pub use message_effects::discord::DiscordEffectsClient;
pub use message_effects::lark_transport::LarkEffectsClient;
pub use message_effects::native_lark::NativeLarkEffectsClient;
pub use message_effects::whatsapp::{ReceivedMessageContext, WhatsappReactionsClient};
pub use message_effects::{MessageEffectReceipt, MessageEffectsCapabilities, MessageEffectsClient};
pub use reply_targets::ReplyTargetResponse;
pub use state::ToolsState;

fn routed_reply_error(error: crate::turns::routed::ExecutionError) -> cmaeg_core::problem::Problem {
    use crate::turns::routed::ExecutionError;
    use cmaeg_core::problem::{Problem, ProblemCode};
    let (code, status, detail) = match error {
        ExecutionError::Abandoned => (ProblemCode::Forbidden, 403, "unknown_abandoned"),
        ExecutionError::IdempotencyConflict => {
            (ProblemCode::Conflict, 409, "reply_idempotency_conflict")
        }
        ExecutionError::OperationLimit => (ProblemCode::RateLimited, 429, "reply_operation_limit"),
        ExecutionError::Busy => (ProblemCode::Conflict, 409, "routed_reply_in_progress"),
        ExecutionError::AuthorityChanged => (ProblemCode::Forbidden, 403, "routed_reply_revoked"),
        ExecutionError::Expired => (ProblemCode::EventRefExpired, 410, "event_ref_expired"),
        ExecutionError::Invalid => (ProblemCode::InvalidRequest, 400, "routed_reply_invalid"),
        _ => (ProblemCode::Internal, 503, "routed_reply_unavailable"),
    };
    let mut problem = Problem::new(code, status, detail);
    if detail == "routed_reply_in_progress" {
        problem.retry_after_seconds = Some(2);
    }
    problem
}

pub(crate) fn validate_reply_key(key: Option<&str>) -> Result<(), cmaeg_core::problem::Problem> {
    if key.is_some_and(|key| {
        key.is_empty() || key.len() > 128 || !key.bytes().all(|byte| (33..=126).contains(&byte))
    }) {
        return Err(cmaeg_core::problem::Problem::new(
            cmaeg_core::problem::ProblemCode::InvalidRequest,
            400,
            "Idempotency-Key must contain 1..128 visible ASCII bytes",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod thread_history_tests;

mod update;

#[cfg(test)]
mod routed_retry_tests {
    #[test]
    fn pending_reply_detail_and_retry_header_remain_stable() {
        let problem = super::routed_reply_error(crate::turns::routed::ExecutionError::Busy);
        assert_eq!(problem.status, 409);
        assert_eq!(problem.detail.as_deref(), Some("routed_reply_in_progress"));
        assert_eq!(problem.retry_after_seconds, Some(2));
        assert!(serde_json::to_value(&problem)
            .unwrap()
            .get("retry_after_seconds")
            .is_none());
    }
}
