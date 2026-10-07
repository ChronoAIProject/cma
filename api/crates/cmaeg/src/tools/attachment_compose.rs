//! Assembling an attachment view from derivation outcomes: the requested
//! text page, images in order within the per-result byte cap, and one
//! notice composed from every part's outcome.
use super::{
    attachment_view::{
        AttachmentImage, AttachmentImageOrigin, AttachmentStatus, AttachmentText,
        AttachmentTextSource, AttachmentUnavailableReason as Reason, AttachmentView,
    },
    attachments::{AttachmentRead, MAX_RESULT_IMAGE_BYTES},
};
use crate::media::{
    derive::{DeriveError, Derived},
    image::NormalizedImage,
    jobs::JobOutcome,
    text::TextDocument,
};
use cmaeg_core::activity::Attachment;
use std::sync::Arc;

/// One derivation's outcome within the call's wait.
pub(super) enum Part {
    Ready(Arc<Derived>),
    Pending(u32),
    Failed(Reason),
}

impl From<JobOutcome<Derived, DeriveError>> for Part {
    fn from(outcome: JobOutcome<Derived, DeriveError>) -> Self {
        match outcome {
            JobOutcome::Ready(derived) => Self::Ready(derived),
            JobOutcome::Pending { retry_after_ms } => {
                Self::Pending(u32::try_from(retry_after_ms).unwrap_or(u32::MAX))
            }
            JobOutcome::Failed(error) => Self::Failed(error.into()),
        }
    }
}

/// A view under construction, with the images selected for it.
pub(super) struct Built {
    pub(super) view: AttachmentView,
    page: u32,
    images: Vec<(AttachmentImageOrigin, NormalizedImage)>,
    pub(super) notices: Vec<String>,
    /// Kept apart so it always comes last.
    transcript_notice: Option<String>,
}

impl Built {
    pub(super) fn new(index: u8, attachment: &Attachment, page: u32) -> Self {
        Self {
            view: AttachmentView {
                index,
                kind: attachment.kind,
                variant: attachment.variant,
                filename: attachment.filename.clone(),
                mime_type: attachment.mime_type.clone(),
                detected_type: None,
                size_bytes: attachment.size_bytes,
                duration_seconds: attachment.duration_seconds,
                width: attachment.width,
                height: attachment.height,
                status: AttachmentStatus::Ready,
                reason: None,
                retry_after_ms: None,
                text: None,
                images: Vec::new(),
                notice: None,
                download: None,
            },
            page,
            images: Vec::new(),
            notices: Vec::new(),
            transcript_notice: None,
        }
    }

    pub(super) fn pending(&mut self, retry_after_ms: u32) {
        self.view.status = AttachmentStatus::Pending;
        self.view.retry_after_ms = Some(retry_after_ms);
    }

    pub(super) fn unavailable(&mut self, reason: Reason) {
        self.view.status = AttachmentStatus::Unavailable;
        self.view.reason = Some(reason);
    }

    /// Use a ready artifact. False when the requested text page is past the
    /// last one; the text still says how many pages exist.
    pub(super) fn take(&mut self, derived: &Derived) -> bool {
        match derived {
            Derived::Image(image) => {
                self.images
                    .push((AttachmentImageOrigin::Normalized, copy(image)));
                true
            }
            Derived::Visuals(frames) => {
                let mut frames: Vec<_> = frames.iter().collect();
                frames.sort_by_key(|frame| frame.at_ms);
                for frame in frames {
                    // Decided per image: a keyframe set can fall back to the
                    // platform thumbnail.
                    let origin = if frame.at_ms.is_some() {
                        AttachmentImageOrigin::Keyframe
                    } else {
                        AttachmentImageOrigin::PlatformThumbnail
                    };
                    self.images.push((origin, copy(frame)));
                }
                true
            }
            Derived::Text(document) => self.text(AttachmentTextSource::Document, document),
            Derived::Transcript {
                document,
                truncated_at_seconds,
                ..
            } => {
                self.transcript_notice = truncated_at_seconds.map(|seconds| {
                    format!(
                        "transcript covers only the first {} of the audio",
                        clock(seconds)
                    )
                });
                self.text(AttachmentTextSource::Transcript, document)
            }
            Derived::Metadata { mime, .. } => {
                self.view.detected_type = Some((*mime).to_owned());
                true
            }
            // Excerpts belong to turn input; no view requests one.
            Derived::Excerpt { .. } => true,
        }
    }

    /// Set the requested page; false when it is past the last page.
    fn text(&mut self, source: AttachmentTextSource, document: &TextDocument) -> bool {
        let pages = u32::try_from(document.pages.len()).unwrap_or(u32::MAX);
        let content = self
            .page
            .checked_sub(1)
            .and_then(|position| usize::try_from(position).ok())
            .and_then(|position| document.pages.get(position));
        self.view.text = Some(AttachmentText {
            source,
            content: content.cloned().unwrap_or_default(),
            page: self.page,
            pages,
            truncated: document.truncated,
        });
        content.is_some()
    }

    /// A video's `auto` view is ready when either part is; the other part's
    /// outcome goes into the notice. When both fail, the transcript's reason
    /// is the view's, because the transcript is the primary content.
    pub(super) fn video(&mut self, frames: Part, transcript: Part) {
        let mut retry = None;
        let mut ready = false;
        let mut failures = Vec::new();
        for (label, part) in [("frames", frames), ("transcript", transcript)] {
            match part {
                Part::Ready(derived) => {
                    if self.take(&derived) {
                        ready = true;
                    } else {
                        self.notices.push(format!("{label} unavailable: not_found"));
                        failures.push(Reason::NotFound);
                    }
                }
                Part::Pending(after) => {
                    self.notices.push(format!("{label} pending"));
                    retry = Some(retry.map_or(after, |current: u32| current.min(after)));
                }
                Part::Failed(reason) => {
                    self.notices
                        .push(format!("{label} unavailable: {}", reason.as_str()));
                    failures.push(reason);
                }
            }
        }
        if ready {
            return;
        }
        if let Some(after) = retry {
            self.pending(after);
        } else if let Some(reason) = failures.last() {
            self.unavailable(*reason);
        }
    }

    /// Select images within [`MAX_RESULT_IMAGE_BYTES`], in order, and
    /// compose the notice.
    pub(super) fn finish(mut self) -> AttachmentRead {
        let mut images = Vec::new();
        let mut raw_image_bytes = 0;
        let offered = self.images.len();
        for (origin, image) in self.images {
            if raw_image_bytes + image.bytes.len() > MAX_RESULT_IMAGE_BYTES {
                break;
            }
            raw_image_bytes += image.bytes.len();
            self.view.images.push(AttachmentImage {
                origin,
                mime_type: image.mime.to_owned(),
                width: image.width,
                height: image.height,
                bytes: u32::try_from(image.bytes.len()).unwrap_or(u32::MAX),
                at_ms: image.at_ms,
            });
            images.push(image);
        }
        let omitted = offered - images.len();
        if omitted > 0 {
            self.view.reason = Some(Reason::ImagesTruncated);
            self.notices.push(format!(
                "{omitted} image(s) omitted: one result carries at most 1 MiB of images"
            ));
        }
        self.notices.extend(self.transcript_notice);
        if !self.notices.is_empty() {
            self.view.notice = Some(self.notices.join("; "));
        }
        AttachmentRead {
            view: self.view,
            images,
            raw_image_bytes,
        }
    }
}

/// The image's bytes are shared, not copied.
fn copy(image: &NormalizedImage) -> NormalizedImage {
    NormalizedImage {
        bytes: image.bytes.clone(),
        mime: image.mime,
        width: image.width,
        height: image.height,
        at_ms: image.at_ms,
    }
}

/// `m:ss` below one hour, `h:mm:ss` from one hour.
fn clock(seconds: u32) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(pages: &[&str], truncated_at_seconds: Option<u32>) -> Derived {
        Derived::Transcript {
            document: TextDocument {
                pages: pages.iter().map(|page| (*page).to_owned()).collect(),
                truncated: false,
            },
            usage_seconds: None,
            truncated_at_seconds,
        }
    }

    fn voice() -> Attachment {
        serde_json::from_value(serde_json::json!({"reference":"r","kind":"audio"})).unwrap()
    }

    #[test]
    fn a_truncated_transcript_carries_the_notice_last() {
        let mut built = Built::new(0, &voice(), 1);
        built.notices.push("first".into());
        assert!(built.take(&transcript(&["words"], Some(600))));
        let read = built.finish();
        assert_eq!(read.view.status, AttachmentStatus::Ready);
        assert_eq!(
            read.view.notice.as_deref(),
            Some("first; transcript covers only the first 10:00 of the audio")
        );
        let mut built = Built::new(0, &voice(), 1);
        assert!(built.take(&transcript(&["words"], Some(3_725))));
        assert_eq!(
            built.finish().view.notice.as_deref(),
            Some("transcript covers only the first 1:02:05 of the audio")
        );
    }

    #[test]
    fn a_page_past_the_last_is_not_found_with_the_page_count() {
        let mut built = Built::new(0, &voice(), 3);
        assert!(!built.take(&transcript(&["one", "two"], None)));
        let view = built.finish().view;
        assert_eq!(view.status, AttachmentStatus::Ready);
        let text = view.text.unwrap();
        assert_eq!((text.page, text.pages, text.content.as_str()), (3, 2, ""));
        assert!(view.notice.is_none());
    }

    #[test]
    fn clock_shows_hours_only_from_one_hour() {
        assert_eq!(clock(59), "0:59");
        assert_eq!(clock(3_599), "59:59");
        assert_eq!(clock(3_600), "1:00:00");
    }
}
