//! The visual-extraction seam: still frames an agent can be shown for a
//! video or animation.
//!
//! Callers depend on [`VisualExtractor`] only. [`PlatformThumbnail`] is the
//! baseline: the preview image a platform already made, normalized like any
//! image. Keyframe extraction is a later extractor that falls back to it.
use super::{
    body::Overflow,
    derive::DeriveError,
    image::{self, NormalizedImage, PREVIEW},
    source::{FetchRequest, SourceCredential, SourceRegistry},
    target::MediaTarget,
};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tokio::{sync::Semaphore, time::Instant};

/// A platform thumbnail is a small preview; anything larger is refused.
pub const THUMBNAIL_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisualKind {
    Thumbnail,
    Keyframes,
}

/// The attachment to extract frames from, and when to give up.
#[derive(Debug, Clone, Copy)]
pub struct VisualRequest<'a> {
    pub target: &'a MediaTarget,
    pub index: usize,
    pub deadline: Instant,
}

/// Frames for an attachment, each normalized to a preview.
pub trait VisualExtractor: Send + Sync {
    fn kind(&self) -> VisualKind;
    fn extract<'a>(
        &'a self,
        request: VisualRequest<'a>,
    ) -> BoxFuture<'a, Result<Vec<NormalizedImage>, DeriveError>>;
}

/// The platform's own thumbnail, which only native Telegram attachments
/// carry, as one preview frame.
pub struct PlatformThumbnail {
    sources: Arc<SourceRegistry>,
    decode: Arc<Semaphore>,
}

impl PlatformThumbnail {
    /// `decode` is the pod's single decode permit, shared with every other
    /// image normalization.
    pub fn new(sources: Arc<SourceRegistry>, decode: Arc<Semaphore>) -> Self {
        Self { sources, decode }
    }
}

impl VisualExtractor for PlatformThumbnail {
    fn kind(&self) -> VisualKind {
        VisualKind::Thumbnail
    }

    fn extract<'a>(
        &'a self,
        request: VisualRequest<'a>,
    ) -> BoxFuture<'a, Result<Vec<NormalizedImage>, DeriveError>> {
        Box::pin(async move {
            let target = request.target;
            let attachment = target
                .attachment(request.index)
                .ok_or(DeriveError::NotFound)?;
            let Some(thumbnail) = attachment.thumbnail.as_ref() else {
                return Err(DeriveError::NotRetrievable);
            };
            let body = self
                .sources
                .fetch(FetchRequest {
                    channel: &target.channel,
                    event: &target.event,
                    activity: &target.activity,
                    locator: Some(thumbnail),
                    declared_size: None,
                    credential: SourceCredential::ReplyToken,
                    cap: THUMBNAIL_MAX_BYTES,
                    overflow: Overflow::Reject,
                    deadline: request.deadline,
                })
                .await?;
            let frame =
                image::normalize(body.bytes, PREVIEW, &self.decode, request.deadline).await?;
            Ok(vec![frame])
        })
    }
}
