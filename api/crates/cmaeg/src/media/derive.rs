//! What can be derived from an attachment, and the one vocabulary of
//! reasons a derivation fails with.
//!
//! The reasons are what agents and operators see, so every lower-level
//! failure (a fetch, a decode, a transcription, a job) maps into
//! [`DeriveError`] here and nowhere else.
use super::{
    cache::Weighted, image::ImageError, image::NormalizedImage, jobs::JobAbort, sniff::Family,
    text::TextDocument, text::TextError, transcribe::TranscribeError, FetchError,
};
use std::fmt;

/// Weight of a [`Derived::Metadata`] entry: a few words of memory, counted
/// so that the cache never holds an unbounded number of free entries.
const METADATA_WEIGHT: u64 = 64;

/// One kind of artifact derived from an attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Artifact {
    Image,
    Text,
    Excerpt,
    Transcript,
    Visuals,
    Metadata,
}

impl Artifact {
    /// The stable name used in cache keys, metrics and spans.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Text => "text",
            Self::Excerpt => "excerpt",
            Self::Transcript => "transcript",
            Self::Visuals => "visuals",
            Self::Metadata => "metadata",
        }
    }
}

/// A derived artifact, as cached.
pub enum Derived {
    Image(NormalizedImage),
    /// The whole decoded document; callers select pages.
    Text(TextDocument),
    Excerpt {
        text: String,
        truncated: bool,
    },
    Transcript {
        document: TextDocument,
        usage_seconds: Option<u32>,
    },
    Visuals(Vec<NormalizedImage>),
    Metadata {
        mime: &'static str,
        family: Family,
    },
}

impl Weighted for Derived {
    fn weight(&self) -> u64 {
        match self {
            Self::Image(image) => image.weight(),
            Self::Text(document) | Self::Transcript { document, .. } => document.weight(),
            Self::Excerpt { text, .. } => text.len() as u64,
            Self::Visuals(frames) => frames.iter().map(Weighted::weight).sum(),
            Self::Metadata { .. } => METADATA_WEIGHT,
        }
    }
}

/// Content must never reach a log line, so only kinds and sizes are shown.
impl fmt::Debug for Derived {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Image(image) => formatter.debug_tuple("Image").field(image).finish(),
            Self::Text(document) => formatter.debug_tuple("Text").field(document).finish(),
            Self::Excerpt { text, truncated } => formatter
                .debug_struct("Excerpt")
                .field("bytes", &text.len())
                .field("truncated", truncated)
                .finish(),
            Self::Transcript {
                document,
                usage_seconds,
            } => formatter
                .debug_struct("Transcript")
                .field("document", document)
                .field("usage_seconds", usage_seconds)
                .finish(),
            Self::Visuals(frames) => formatter.debug_tuple("Visuals").field(frames).finish(),
            Self::Metadata { mime, family } => formatter
                .debug_struct("Metadata")
                .field("mime", mime)
                .field("family", family)
                .finish(),
        }
    }
}

/// Why an artifact could not be derived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeriveError {
    NotRetrievable,
    NotFound,
    TooLarge,
    TooLargePixels,
    UnsupportedFormat,
    DecodeFailed,
    TimedOut,
    TranscriberDisabled,
    TranscriberFailed,
    /// Media longer than a transcriber accepts.
    TooLong,
    RateLimited,
    /// The upstream answered 5xx, still refused the credential after its
    /// retry, or failed in transport.
    SourceUnavailable,
    /// A sandboxed transform failed.
    TransformFailed,
}

impl DeriveError {
    /// The agent-facing reason.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotRetrievable => "not_retrievable",
            Self::NotFound => "not_found",
            Self::TooLarge => "too_large",
            Self::TooLargePixels => "too_large_pixels",
            Self::UnsupportedFormat => "unsupported_format",
            Self::DecodeFailed => "decode_failed",
            Self::TimedOut => "timed_out",
            Self::TranscriberDisabled => "transcriber_disabled",
            Self::TranscriberFailed => "transcriber_failed",
            Self::TooLong => "too_long",
            Self::RateLimited => "rate_limited",
            Self::SourceUnavailable => "source_unavailable",
            Self::TransformFailed => "transform_failed",
        }
    }
}

impl fmt::Display for DeriveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::error::Error for DeriveError {}

impl From<FetchError> for DeriveError {
    fn from(error: FetchError) -> Self {
        match error {
            FetchError::NotRetrievable => Self::NotRetrievable,
            FetchError::TooLarge => Self::TooLarge,
            FetchError::NotFound => Self::NotFound,
            FetchError::TimedOut => Self::TimedOut,
            FetchError::RateLimited => Self::RateLimited,
            // The agent can do nothing different about any of these: the
            // source is, for now, not giving the bytes.
            FetchError::Busy
            | FetchError::Unauthorized
            | FetchError::Rejected
            | FetchError::Upstream => Self::SourceUnavailable,
        }
    }
}

impl From<ImageError> for DeriveError {
    fn from(error: ImageError) -> Self {
        match error {
            ImageError::UnsupportedFormat => Self::UnsupportedFormat,
            ImageError::TooLargePixels => Self::TooLargePixels,
            ImageError::DecodeFailed => Self::DecodeFailed,
            ImageError::TooLarge => Self::TooLarge,
            ImageError::TimedOut => Self::TimedOut,
        }
    }
}

impl From<TextError> for DeriveError {
    fn from(error: TextError) -> Self {
        match error {
            TextError::NotText => Self::UnsupportedFormat,
        }
    }
}

impl From<JobAbort> for DeriveError {
    fn from(abort: JobAbort) -> Self {
        match abort {
            JobAbort::TimedOut => Self::TimedOut,
            JobAbort::Busy => Self::RateLimited,
        }
    }
}

impl From<TranscribeError> for DeriveError {
    fn from(error: TranscribeError) -> Self {
        match error {
            TranscribeError::Disabled => Self::TranscriberDisabled,
            TranscribeError::Unsupported => Self::UnsupportedFormat,
            TranscribeError::TimedOut => Self::TimedOut,
            TranscribeError::RateLimited => Self::RateLimited,
            TranscribeError::Failed => Self::TranscriberFailed,
        }
    }
}
