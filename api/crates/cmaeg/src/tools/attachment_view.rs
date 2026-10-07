//! The wire shape of one attachment read, shared by `readEventAttachment`
//! over MCP and its REST twin.
//!
//! A view carries facts and status only. Image bytes never appear here: MCP
//! sends them as separate image content items and REST never sends them.
use cmaeg_core::activity::{AttachmentKind, MediaVariant};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What the caller asks to read from an attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentViewKind {
    /// The most useful view for the attachment's kind.
    Auto,
    Image,
    Text,
    Frames,
    Download,
}

impl AttachmentViewKind {
    /// The stable label used in metrics and spans.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Image => "image",
            Self::Text => "text",
            Self::Frames => "frames",
            Self::Download => "download",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentStatus {
    Ready,
    /// Still being produced; call again after `retry_after_ms`.
    Pending,
    Unavailable,
}

impl AttachmentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Pending => "pending",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Why a view is unavailable, pending on the response budget, or ready with
/// some images left out. Every value is a fixed code, so it is safe in logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentUnavailableReason {
    NotRetrievable,
    NotFound,
    TooLarge,
    TooLargePixels,
    UnsupportedFormat,
    DecodeFailed,
    TimedOut,
    SourceUnavailable,
    TranscriberDisabled,
    TranscriberFailed,
    TooLong,
    RateLimited,
    TransformFailed,
    DownloadNotPermitted,
    LinkUnavailable,
    ImagesTruncated,
    ResponseBudget,
}

impl AttachmentUnavailableReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotRetrievable => "not_retrievable",
            Self::NotFound => "not_found",
            Self::TooLarge => "too_large",
            Self::TooLargePixels => "too_large_pixels",
            Self::UnsupportedFormat => "unsupported_format",
            Self::DecodeFailed => "decode_failed",
            Self::TimedOut => "timed_out",
            Self::SourceUnavailable => "source_unavailable",
            Self::TranscriberDisabled => "transcriber_disabled",
            Self::TranscriberFailed => "transcriber_failed",
            Self::TooLong => "too_long",
            Self::RateLimited => "rate_limited",
            Self::TransformFailed => "transform_failed",
            Self::DownloadNotPermitted => "download_not_permitted",
            Self::LinkUnavailable => "link_unavailable",
            Self::ImagesTruncated => "images_truncated",
            Self::ResponseBudget => "response_budget",
        }
    }
}

/// Where a page of text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentTextSource {
    Transcript,
    Document,
}

/// How an image was produced. It is decided per image, because one set of
/// frames can mix extracted keyframes with a platform thumbnail fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentImageOrigin {
    Normalized,
    PlatformThumbnail,
    Keyframe,
}

/// One page of a transcript or of a document's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentText {
    pub source: AttachmentTextSource,
    pub content: String,
    /// 1-based page number of `content`.
    pub page: u32,
    pub pages: u32,
    /// The transcript or document text was itself cut at its cap.
    pub truncated: bool,
}

/// Metadata of one image delivered with the view; never its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentImage {
    pub origin: AttachmentImageOrigin,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u32,
    /// The frame's position in the video, for keyframes only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<u64>,
}

/// One attachment's facts and the requested view of its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentView {
    pub index: u8,
    pub kind: AttachmentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<MediaVariant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// The MIME type sniffed from the bytes, when it was determined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub status: AttachmentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<AttachmentUnavailableReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<AttachmentText>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<AttachmentImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}
