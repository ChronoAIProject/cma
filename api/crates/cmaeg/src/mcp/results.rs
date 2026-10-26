//! Content-only MCP results for attachment reads.
//!
//! Codex turns MCP `image` content into model image input only when the
//! result has no `structuredContent`; with it, the model sees only that JSON
//! as text. An attachment result is therefore a text item carrying the view
//! followed by one image item per image, and nothing else.
use crate::{
    media::{budget::ResponseBudget, forward::PermitSet, image::NormalizedImage},
    tools::{
        AttachmentRead, AttachmentStatus, AttachmentUnavailableReason, AttachmentView,
        BUDGET_RETRY_MS, REST_VIEW_WEIGHT,
    },
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use tokio::time::Instant;

/// What a tool call answers with.
pub enum ToolOutput {
    /// A JSON result, serialized the usual way: a tool's structured
    /// envelope, or the result of any other method.
    Envelope(Value),
    /// A content-only result whose body is serialized once and forwarded
    /// while `permits` hold its share of the response budget.
    Content { result: Value, permits: PermitSet },
}

/// The MCP result of `read`, budgeted on `responses`.
///
/// The weight covers the decoded images, their base64 text and the
/// serialized response; a result without images weighs what a REST view
/// does. When no budget frees before `deadline`, the images are dropped and
/// a small text-only `pending` view that holds no permit is returned.
pub async fn attachment_output(
    responses: &ResponseBudget,
    read: AttachmentRead,
    deadline: Instant,
) -> ToolOutput {
    let raw = u64::try_from(read.raw_image_bytes).unwrap_or(u64::MAX);
    let weight = if read.images.is_empty() {
        REST_VIEW_WEIGHT
    } else {
        raw.saturating_mul(3).saturating_add(REST_VIEW_WEIGHT)
    };
    let Some(permit) = responses.acquire(weight, deadline).await else {
        tracing::debug!(
            weight,
            "attachment result deferred: response budget exhausted"
        );
        return ToolOutput::Content {
            result: content(&budget_pending(read.view), &[]),
            permits: PermitSet::new(),
        };
    };
    let mut permits = PermitSet::new();
    permits.push(permit);
    ToolOutput::Content {
        result: content(&read.view, &read.images),
        permits,
    }
}

/// The view with only its facts, waiting for the response budget.
fn budget_pending(mut view: AttachmentView) -> AttachmentView {
    view.status = AttachmentStatus::Pending;
    view.reason = Some(AttachmentUnavailableReason::ResponseBudget);
    view.retry_after_ms = Some(BUDGET_RETRY_MS);
    view.text = None;
    view.images.clear();
    view.notice = None;
    // A deferred result never reaches the agent, so neither does its link.
    view.download = None;
    view
}

/// `{"content":[text, image…],"isError":false}`, never with
/// `structuredContent`. A single image is shown in high detail; several
/// let the client choose.
fn content(view: &AttachmentView, images: &[NormalizedImage]) -> Value {
    let detail = if images.len() == 1 { "high" } else { "auto" };
    let mut items = Vec::with_capacity(images.len() + 1);
    items.push(json!({"type": "text", "text": json!(view).to_string()}));
    items.extend(images.iter().map(|image| {
        json!({
            "type": "image",
            "data": STANDARD.encode(&image.bytes),
            "mimeType": image.mime,
            "_meta": {"codex/imageDetail": detail},
        })
    }));
    json!({"content": items, "isError": false})
}
