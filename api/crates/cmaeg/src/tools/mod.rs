//! Shared authenticated tool operations used by REST and MCP.
mod attachment_compose;
mod attachment_view;
mod attachments;
mod context;
mod download_access;
mod media_links;
pub(crate) mod message_effects;
mod reply_targets;
mod state;
pub use attachment_view::{
    AttachmentImage, AttachmentImageOrigin, AttachmentStatus, AttachmentText, AttachmentTextSource,
    AttachmentUnavailableReason, AttachmentView, AttachmentViewKind, InboundMediaCapabilities,
    MediaDownload, VisualSupport,
};
pub use attachments::{
    valid_request, AttachmentRead, BUDGET_ACQUIRE_MAX, BUDGET_RETRY_MS, MAX_INDEX, MAX_PAGE,
    MAX_RESULT_IMAGE_BYTES, REST_VIEW_WEIGHT, TOOL_WAIT,
};
pub use context::{
    Capabilities, ConsumerSummary, ContextResponse, NativeMediaCapabilities, PublicActivity,
};
pub(crate) use download_access::{Authorized, Refusal};
pub(crate) use media_links::safe_name;
pub use media_links::{
    LinkRefusal, MediaDownloads, DOWNLOAD_RESPONSE_WEIGHT, DOWNLOAD_STREAMS_GLOBAL,
    DOWNLOAD_STREAMS_PER_CHANNEL,
};
pub use message_effects::{MessageEffectReceipt, MessageEffectsCapabilities, MessageEffectsClient};
pub use reply_targets::ReplyTargetResponse;
pub use state::ToolsState;

fn routed_reply_error(error: crate::turns::routed::ExecutionError) -> cmaeg_core::problem::Problem {
    use crate::turns::routed::ExecutionError;
    use cmaeg_core::problem::{Problem, ProblemCode};
    let (code, status, detail) = match error {
        ExecutionError::Busy => (ProblemCode::Conflict, 409, "routed_reply_in_progress"),
        ExecutionError::AuthorityChanged => (ProblemCode::Forbidden, 403, "routed_reply_revoked"),
        ExecutionError::Expired => (ProblemCode::EventRefExpired, 410, "event_ref_expired"),
        ExecutionError::Invalid => (ProblemCode::InvalidRequest, 400, "routed_reply_invalid"),
        _ => (ProblemCode::Internal, 503, "routed_reply_unavailable"),
    };
    Problem::new(code, status, detail)
}
