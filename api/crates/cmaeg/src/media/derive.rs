//! What can be derived from an attachment, how each artifact is produced,
//! and the one vocabulary of reasons a derivation fails with.
//!
//! The reasons are what agents and operators see, so every lower-level
//! failure (a fetch, a decode, a transcription, a job) maps into
//! [`DeriveError`] here and nowhere else.
use super::{
    body::{FetchedBody, Overflow},
    budget::{InflightBytes, InflightPermit},
    cache::Weighted,
    image::{self, ImageError, NormalizedImage, FULL},
    jobs::JobAbort,
    sniff::{sniff, Family, Sniffed},
    source::{FetchRequest, SourceCredential, SourceRegistry},
    target::MediaTarget,
    text::{self, TextDocument, TextError},
    transcribe::{
        meter::SttSpend,
        upload::{transcript_pipeline, AudioTranscoder},
        TranscribeError, Transcriber,
    },
    visual::{VisualExtractor, VisualKind, VisualRequest, THUMBNAIL_MAX_BYTES},
    FetchError, MediaLimits,
};
use std::{fmt, sync::Arc};
use tokio::{sync::Semaphore, time::Instant};

/// Bytes read to classify an attachment, and the head every sniff inspects.
const SNIFF_BYTES: usize = 4096;

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
        /// Set when only the first N seconds were transcribed.
        truncated_at_seconds: Option<u32>,
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
                truncated_at_seconds,
            } => formatter
                .debug_struct("Transcript")
                .field("document", document)
                .field("usage_seconds", usage_seconds)
                .field("truncated_at_seconds", truncated_at_seconds)
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

/// Everything a derivation job needs, shared by every job of the pod.
///
/// The sniffed type of the fetched bytes always decides: a declared image
/// whose bytes are text fails as an image and succeeds as text.
pub struct Pipeline {
    pub sources: Arc<SourceRegistry>,
    pub transcriber: Arc<dyn Transcriber>,
    /// How a fetched original becomes the transcriber's upload.
    pub transcoder: Arc<dyn AudioTranscoder>,
    /// The speech-to-text call rate and hourly budgets.
    pub stt: Arc<SttSpend>,
    pub visuals: Arc<dyn VisualExtractor>,
    pub limits: MediaLimits,
    /// The single decode permit, shared with the visual extractor.
    pub decode: Arc<Semaphore>,
    pub inflight: InflightBytes,
}

impl Pipeline {
    /// Produce `artifact` for attachment `index` before `deadline`.
    pub async fn run(
        &self,
        target: &MediaTarget,
        index: usize,
        artifact: Artifact,
        deadline: Instant,
    ) -> Result<Derived, DeriveError> {
        match artifact {
            Artifact::Image => {
                let (body, _held) = self
                    .fetch(
                        target,
                        index,
                        self.limits.max_bytes,
                        Overflow::Reject,
                        deadline,
                    )
                    .await?;
                expect(&body, &[Family::Image])?;
                let normalized = image::normalize(body.bytes, FULL, &self.decode, deadline).await?;
                Ok(Derived::Image(normalized))
            }
            Artifact::Text => {
                let (body, _held) = self
                    .fetch(
                        target,
                        index,
                        text::TEXT_READ_LIMIT,
                        Overflow::Truncate,
                        deadline,
                    )
                    .await?;
                expect(&body, &[Family::Text])?;
                Ok(Derived::Text(text::decode(&body)?))
            }
            Artifact::Excerpt => {
                let (body, _held) = self
                    .fetch(
                        target,
                        index,
                        text::EXCERPT_READ_LIMIT,
                        Overflow::Truncate,
                        deadline,
                    )
                    .await?;
                expect(&body, &[Family::Text])?;
                let (text, truncated) = text::excerpt(&text::decode(&body)?, text::EXCERPT_BYTES);
                Ok(Derived::Excerpt { text, truncated })
            }
            Artifact::Transcript => transcript_pipeline(self, target, index, deadline).await,
            Artifact::Visuals => {
                // The extractor fetches for itself; its largest fetch is held
                // against the pod-wide budget for as long as it runs.
                let bound = match self.visuals.kind() {
                    VisualKind::Thumbnail => THUMBNAIL_MAX_BYTES,
                    VisualKind::Keyframes => self.limits.max_bytes,
                };
                let _held = self.reserve(bound, deadline).await?;
                let frames = self
                    .visuals
                    .extract(VisualRequest {
                        target,
                        index,
                        deadline,
                    })
                    .await?;
                Ok(Derived::Visuals(frames))
            }
            Artifact::Metadata => {
                let cap = SNIFF_BYTES as u64;
                let (body, _held) = self
                    .fetch(target, index, cap, Overflow::Truncate, deadline)
                    .await?;
                let Sniffed { mime, family } = sniff_body(&body);
                Ok(Derived::Metadata { mime, family })
            }
        }
    }

    /// Fetch the attachment's bytes under `cap`, holding a pod-wide
    /// reservation for them that the caller keeps until its transform ends.
    pub(in crate::media) async fn fetch(
        &self,
        target: &MediaTarget,
        index: usize,
        cap: u64,
        overflow: Overflow,
        deadline: Instant,
    ) -> Result<(FetchedBody, InflightPermit), DeriveError> {
        let attachment = target.attachment(index).ok_or(DeriveError::NotFound)?;
        let declared = attachment.size_bytes.unwrap_or(cap).min(cap);
        let held = self.reserve(declared, deadline).await?;
        let body = self
            .sources
            .fetch(FetchRequest {
                channel: &target.channel,
                event: &target.event,
                activity: &target.activity,
                locator: attachment.source.as_ref(),
                declared_size: attachment.size_bytes,
                credential: SourceCredential::ReplyToken,
                cap,
                overflow,
                deadline,
            })
            .await?;
        Ok((body, held))
    }

    async fn reserve(&self, bytes: u64, deadline: Instant) -> Result<InflightPermit, DeriveError> {
        self.inflight
            .acquire(bytes, deadline)
            .await
            .ok_or(DeriveError::TimedOut)
    }
}

fn sniff_body(body: &FetchedBody) -> Sniffed {
    sniff(&body.bytes[..body.bytes.len().min(SNIFF_BYTES)])
}

/// The body's sniffed type, when its family is one of `families`.
pub(in crate::media) fn expect(
    body: &FetchedBody,
    families: &[Family],
) -> Result<Sniffed, DeriveError> {
    let sniffed = sniff_body(body);
    if families.contains(&sniffed.family) {
        Ok(sniffed)
    } else {
        Err(DeriveError::UnsupportedFormat)
    }
}
