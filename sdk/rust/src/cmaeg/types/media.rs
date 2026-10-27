//! Attachment read wire types for the independent CMAEG v1 protocol.
use super::{AttachmentKind, MediaVariant};
use serde::{Deserialize, Serialize};

/// Native Lark/Feishu message resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LarkResourceKind {
    /// Image resource.
    Image,
    /// File resource, including embedded video.
    File,
}

/// CMAEG `AttachmentViewKind` wire shape: what to read from an attachment.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentViewKind {
    /// Wire value `auto`.
    #[serde(rename = "auto")]
    Auto,
    /// Wire value `image`.
    #[serde(rename = "image")]
    Image,
    /// Wire value `text`.
    #[serde(rename = "text")]
    Text,
    /// Wire value `frames`.
    #[serde(rename = "frames")]
    Frames,
    /// Wire value `download`.
    #[serde(rename = "download")]
    Download,
}

/// CMAEG `AttachmentStatus` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentStatus {
    /// Wire value `ready`.
    #[serde(rename = "ready")]
    Ready,
    /// Wire value `pending`.
    #[serde(rename = "pending")]
    Pending,
    /// Wire value `unavailable`.
    #[serde(rename = "unavailable")]
    Unavailable,
}

/// CMAEG `AttachmentUnavailableReason` wire shape: why a view is unavailable, pending on the response budget, or missing images.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentUnavailableReason {
    /// Wire value `not_retrievable`.
    #[serde(rename = "not_retrievable")]
    NotRetrievable,
    /// Wire value `not_found`.
    #[serde(rename = "not_found")]
    NotFound,
    /// Wire value `too_large`.
    #[serde(rename = "too_large")]
    TooLarge,
    /// Wire value `too_large_pixels`.
    #[serde(rename = "too_large_pixels")]
    TooLargePixels,
    /// Wire value `unsupported_format`.
    #[serde(rename = "unsupported_format")]
    UnsupportedFormat,
    /// Wire value `decode_failed`.
    #[serde(rename = "decode_failed")]
    DecodeFailed,
    /// Wire value `timed_out`.
    #[serde(rename = "timed_out")]
    TimedOut,
    /// Wire value `source_unavailable`.
    #[serde(rename = "source_unavailable")]
    SourceUnavailable,
    /// Wire value `transcriber_disabled`.
    #[serde(rename = "transcriber_disabled")]
    TranscriberDisabled,
    /// Wire value `transcriber_failed`.
    #[serde(rename = "transcriber_failed")]
    TranscriberFailed,
    /// Wire value `too_long`.
    #[serde(rename = "too_long")]
    TooLong,
    /// Wire value `rate_limited`.
    #[serde(rename = "rate_limited")]
    RateLimited,
    /// Wire value `transform_failed`.
    #[serde(rename = "transform_failed")]
    TransformFailed,
    /// Wire value `download_not_permitted`.
    #[serde(rename = "download_not_permitted")]
    DownloadNotPermitted,
    /// Wire value `link_unavailable`.
    #[serde(rename = "link_unavailable")]
    LinkUnavailable,
    /// Wire value `images_truncated`.
    #[serde(rename = "images_truncated")]
    ImagesTruncated,
    /// Wire value `response_budget`.
    #[serde(rename = "response_budget")]
    ResponseBudget,
}

/// CMAEG `AttachmentTextSource` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentTextSource {
    /// Wire value `transcript`.
    #[serde(rename = "transcript")]
    Transcript,
    /// Wire value `document`.
    #[serde(rename = "document")]
    Document,
}

/// CMAEG `AttachmentImageOrigin` wire shape, decided per image.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentImageOrigin {
    /// Wire value `normalized`.
    #[serde(rename = "normalized")]
    Normalized,
    /// Wire value `platform_thumbnail`.
    #[serde(rename = "platform_thumbnail")]
    PlatformThumbnail,
    /// Wire value `keyframe`.
    #[serde(rename = "keyframe")]
    Keyframe,
}

/// CMAEG `AttachmentText` wire shape: one page of a transcript or document.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentText {
    /// Wire field `content`.
    pub content: String,
    /// Wire field `page`, 1-based.
    pub page: u64,
    /// Wire field `pages`.
    pub pages: u64,
    /// Wire field `source`.
    pub source: AttachmentTextSource,
    /// Wire field `truncated`.
    pub truncated: bool,
}

/// CMAEG `AttachmentImage` wire shape: metadata of one delivered image, never its bytes.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentImage {
    /// Wire field `at_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<u64>,
    /// Wire field `bytes`.
    pub bytes: u64,
    /// Wire field `height`.
    pub height: u64,
    /// Wire field `mime_type`.
    pub mime_type: String,
    /// Wire field `origin`.
    pub origin: AttachmentImageOrigin,
    /// Wire field `width`.
    pub width: u64,
}

/// CMAEG `MediaDownload` wire shape: a ready download as one `curl` command
/// that works once and expires at `expires_at`. Only `curl` carries the
/// single-use link.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaDownload {
    /// Wire field `curl`.
    pub curl: String,
    /// Wire field `expires_at`.
    pub expires_at: i64,
    /// Wire field `method`.
    pub method: String,
    /// Wire field `url`.
    pub url: String,
}

/// CMAEG `AttachmentView` wire shape: one attachment's facts and the requested view.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentView {
    /// Wire field `detected_type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_type: Option<String>,
    /// Wire field `download`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<MediaDownload>,
    /// Wire field `duration_seconds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    /// Wire field `filename`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Wire field `height`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u64>,
    /// Wire field `images`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<AttachmentImage>,
    /// Wire field `index`.
    pub index: u64,
    /// Wire field `kind`.
    pub kind: AttachmentKind,
    /// Wire field `mime_type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Wire field `notice`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    /// Wire field `reason`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<AttachmentUnavailableReason>,
    /// Wire field `retry_after_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Wire field `size_bytes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Wire field `status`.
    pub status: AttachmentStatus,
    /// Wire field `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<AttachmentText>,
    /// Wire field `variant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<MediaVariant>,
    /// Wire field `width`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u64>,
}

/// CMAEG `VisualSupport` wire shape: the still images a gateway can show for
/// a video.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VisualSupport {
    /// Wire value `none`.
    #[serde(rename = "none")]
    None,
    /// Wire value `platform_thumbnail`.
    #[serde(rename = "platform_thumbnail")]
    PlatformThumbnail,
    /// Wire value `keyframes`.
    #[serde(rename = "keyframes")]
    Keyframes,
}

/// CMAEG `InboundMediaCapabilities` wire shape: what the calling binding can
/// do with an event's inbound attachments.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct InboundMediaCapabilities {
    /// Wire field `downloads`.
    pub downloads: bool,
    /// Wire field `read_tool`.
    pub read_tool: bool,
    /// Wire field `transcripts`.
    pub transcripts: bool,
    /// Wire field `visuals`.
    pub visuals: VisualSupport,
}
