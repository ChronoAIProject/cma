//! Shared authenticated tool operations used by REST and MCP.
mod attachment_compose;
mod attachment_view;
mod attachments;
mod context;
mod download_access;
mod media_links;
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
pub use context::{Capabilities, ConsumerSummary, ContextResponse, PublicActivity};
pub(crate) use download_access::{Authorized, Refusal};
pub(crate) use media_links::safe_name;
pub use media_links::{
    LinkRefusal, MediaDownloads, DOWNLOAD_RESPONSE_WEIGHT, DOWNLOAD_STREAMS_GLOBAL,
    DOWNLOAD_STREAMS_PER_CHANNEL,
};
pub use reply_targets::ReplyTargetResponse;
pub use state::ToolsState;
