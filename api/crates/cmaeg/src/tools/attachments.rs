//! `readEventAttachment`: one attachment of an inbound event, read through
//! the media service with its authority re-verified on every call.
//!
//! The attachment list and every locator come from the provider's persisted
//! activity, so a read always requires the event_ref's sealed digest to
//! verify that activity. Work that outlasts the tool budget is answered as
//! `pending` while its job finishes into the derived cache.
use super::{
    attachment_compose::{Built, Part},
    attachment_view::{AttachmentUnavailableReason as Reason, AttachmentView, AttachmentViewKind},
    context, LinkRefusal, ToolsState,
};
use crate::{
    auth::AgentCaller,
    media::{
        derive::{Artifact, DeriveError, Derived},
        image::NormalizedImage,
        service::MediaService,
        sniff::Family,
        target::MediaTarget,
    },
    telemetry::{self, names, operations::Operation},
    turns::render_media,
};
use cmaeg_core::{
    activity::{Attachment, AttachmentKind, MAX_ATTACHMENTS},
    problem::{Problem, ProblemCode},
    seal::EventRef,
};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;
use tracing::{field::Empty, Instrument};

/// Raw image bytes per MCP result. About 1.4 MiB once base64-encoded and
/// serialized, so four to five results fit layer 4's 8 MiB journal of
/// unacknowledged frames, under its 4 MiB single-frame cap.
pub const MAX_RESULT_IMAGE_BYTES: usize = 1_048_576;
/// Compute budget for one call. It leaves time to serialize the result
/// within the sandbox tool proxy's 10 s upstream deadline.
pub const TOOL_WAIT: Duration = Duration::from_secs(8);
/// `retry_after_ms` when the response budget is exhausted: permits free as
/// soon as in-flight bodies finish, which is usually well under a second.
pub const BUDGET_RETRY_MS: u32 = 1_000;
/// Upper bound on waiting for a response-budget permit; the deadline is the
/// smaller of this and what remains of [`TOOL_WAIT`].
pub const BUDGET_ACQUIRE_MAX: Duration = Duration::from_secs(2);
/// Response-budget weight of a REST view, or of an MCP result without
/// images: a page of text is at most 16 KiB, so this covers its JSON with
/// room for escaping.
pub const REST_VIEW_WEIGHT: u64 = 131_072;
/// Text reads are capped at 1 MiB in 16 KiB pages.
pub const MAX_PAGE: u32 = 64;
/// The highest attachment index an activity can carry.
pub const MAX_INDEX: u8 = (MAX_ATTACHMENTS - 1) as u8;

const BINARY_NOTICE: &str = "binary file: only metadata is available to this agent";
const NO_FRAMES_NOTICE: &str =
    "frames are not available for this attachment on this gateway; the transcript is";
const USE_IMAGE_NOTICE: &str = "use view \"image\"";
const NOT_PERMITTED_NOTICE: &str =
    "downloads are available only to autonomous agents with gateway tools";
const DOWNLOAD_NOTICE: &str =
    "the command works once and expires at expires_at; run it in your workspace";
const USE_FRAMES_NOTICE: &str = "use view \"frames\"";

/// A view and the image bytes that belong with it. Only MCP sends the bytes;
/// the view holds their metadata.
pub struct AttachmentRead {
    pub view: AttachmentView,
    pub images: Vec<NormalizedImage>,
    pub raw_image_bytes: usize,
}

/// Whether an attachment index and a text page are within the protocol's
/// bounds. Out-of-bounds values are malformed requests, not missing items.
pub fn valid_request(index: u8, page: u32) -> bool {
    index <= MAX_INDEX && (1..=MAX_PAGE).contains(&page)
}

/// Every derivation failure has exactly one agent-facing reason. The match
/// has no wildcard arm, so a new failure fails to compile until mapped.
impl From<DeriveError> for Reason {
    fn from(error: DeriveError) -> Self {
        match error {
            DeriveError::NotRetrievable => Self::NotRetrievable,
            DeriveError::NotFound => Self::NotFound,
            DeriveError::TooLarge => Self::TooLarge,
            DeriveError::TooLargePixels => Self::TooLargePixels,
            DeriveError::UnsupportedFormat => Self::UnsupportedFormat,
            DeriveError::DecodeFailed => Self::DecodeFailed,
            DeriveError::TimedOut => Self::TimedOut,
            DeriveError::SourceUnavailable => Self::SourceUnavailable,
            DeriveError::TranscriberDisabled => Self::TranscriberDisabled,
            DeriveError::TranscriberFailed => Self::TranscriberFailed,
            DeriveError::TooLong => Self::TooLong,
            DeriveError::RateLimited => Self::RateLimited,
            DeriveError::TransformFailed => Self::TransformFailed,
        }
    }
}

impl ToolsState {
    /// Read `view` of attachment `index` of the event `reference` names.
    pub async fn read_attachment(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        index: u8,
        view: AttachmentViewKind,
        page: u32,
    ) -> Result<AttachmentRead, Problem> {
        let started = Instant::now();
        let mut observation =
            Operation::new(names::TOOL_DURATION, names::TOOL, "readEventAttachment");
        let span = tracing::info_span!(
            "cmaeg.media.read",
            channel_hash = Empty,
            view = view.as_str(),
            status = Empty,
            reason = Empty,
        );
        let result = self
            .read_verified(caller, reference, index, view, page, started)
            .instrument(span.clone())
            .await;
        let (status, reason) = match &result {
            Ok(read) => (read.view.status.as_str(), read.view.reason),
            Err(_) => ("refused", None),
        };
        span.record("status", status);
        if let Some(reason) = reason {
            span.record("reason", reason.as_str());
        }
        span.in_scope(|| match &result {
            Ok(_) => tracing::debug!("attachment read"),
            Err(problem) => tracing::info!(code = %problem.code, "attachment read refused"),
        });
        metrics::counter!(
            names::MEDIA_TOOL_READS,
            names::VIEW => view.as_str(),
            names::STATUS => status
        )
        .increment(1);
        observation.finish(if result.is_ok() { "success" } else { "error" });
        result
    }

    async fn read_verified(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        index: u8,
        view: AttachmentViewKind,
        page: u32,
        started: Instant,
    ) -> Result<AttachmentRead, Problem> {
        let media = self
            .media
            .clone()
            .ok_or_else(|| Problem::new(ProblemCode::MediaUnavailable, 503, "media_disabled"))?;
        if !valid_request(index, page) {
            return Err(Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "invalid attachment request",
            ));
        }
        let (channel, event) = self.authorize_event(caller, reference).await?;
        tracing::Span::current().record(
            "channel_hash",
            telemetry::channel_hash(channel.id.as_str()).as_str(),
        );
        let consumer = context::default_consumer(&channel)?;
        let (activity, verified) =
            context::read_activity(self, &channel, &consumer, &event).await?;
        // Media always needs a verified digest, even for an event_ref sealed
        // without one: the locators must be the ones the source delivered.
        if !verified {
            return Err(unverified());
        }
        let Some(attachment) = render_media::attachments(&activity)
            .get(usize::from(index))
            .cloned()
        else {
            return Err(Problem::new(
                ProblemCode::AttachmentNotFound,
                404,
                "attachment_not_found",
            ));
        };
        let target =
            MediaTarget::new(channel, event, Arc::new(activity)).map_err(|_| unverified())?;
        let reader = Reader {
            tools: self,
            media: &media,
            target: &target,
            attachment: &attachment,
            index,
            page,
            started,
        };
        Ok(reader.read(view).await)
    }
}

fn unverified() -> Problem {
    Problem::new(
        ProblemCode::MediaUnavailable,
        503,
        "event_context_unverified",
    )
}

/// The artifacts a view needs, decided by the attachment's family.
enum Plan {
    /// A single-use link to the original.
    Link,
    Derive(Artifact),
    /// A video's frames and transcript, derived concurrently.
    VideoAuto,
    Unsupported(Option<&'static str>),
    MetadataOnly,
}

fn plan(family: Family, view: AttachmentViewKind) -> Plan {
    use AttachmentViewKind as View;
    match (family, view) {
        (_, View::Download) => Plan::Link,
        (Family::Image, View::Auto | View::Image) => Plan::Derive(Artifact::Image),
        (Family::Image, View::Text | View::Frames) => Plan::Unsupported(Some(USE_IMAGE_NOTICE)),
        (Family::Audio, View::Auto | View::Text) => Plan::Derive(Artifact::Transcript),
        (Family::Video, View::Auto) => Plan::VideoAuto,
        (Family::Video, View::Text) => Plan::Derive(Artifact::Transcript),
        (Family::Video, View::Frames) => Plan::Derive(Artifact::Visuals),
        (Family::Video, View::Image) => Plan::Unsupported(Some(USE_FRAMES_NOTICE)),
        (Family::Text, View::Auto | View::Text) => Plan::Derive(Artifact::Text),
        (Family::Other, View::Auto) => Plan::MetadataOnly,
        (Family::Audio, View::Image | View::Frames)
        | (Family::Text, View::Image | View::Frames)
        | (Family::Other, View::Image | View::Text | View::Frames) => Plan::Unsupported(None),
    }
}

struct Reader<'a> {
    tools: &'a ToolsState,
    media: &'a MediaService,
    target: &'a MediaTarget,
    attachment: &'a Attachment,
    index: u8,
    page: u32,
    started: Instant,
}

impl Reader<'_> {
    async fn derive(&self, artifact: Artifact) -> Part {
        let wait = TOOL_WAIT.saturating_sub(self.started.elapsed());
        self.media
            .derive(self.target, usize::from(self.index), artifact, wait)
            .await
            .into()
    }

    /// Mint the download link, or say why there is none.
    fn link(&self, built: &mut Built) {
        if self.attachment.source.is_none() {
            built.unavailable(Reason::NotRetrievable);
            return;
        }
        let target = self.target;
        match self
            .tools
            .mint_link(&target.channel, &target.event, self.attachment, self.index)
        {
            Ok(download) => {
                built.view.download = Some(download);
                built.notices.push(DOWNLOAD_NOTICE.into());
            }
            Err(LinkRefusal::NotPermitted) => {
                built.notices.push(NOT_PERMITTED_NOTICE.into());
                built.unavailable(Reason::DownloadNotPermitted);
            }
            Err(LinkRefusal::RateLimited) => built.unavailable(Reason::RateLimited),
            Err(LinkRefusal::Unavailable) => built.unavailable(Reason::LinkUnavailable),
        }
    }

    async fn read(&self, view: AttachmentViewKind) -> AttachmentRead {
        let mut built = Built::new(self.index, self.attachment, self.page);
        // A download needs no derivation, not even a file's sniffed type.
        if view == AttachmentViewKind::Download {
            self.link(&mut built);
            return built.finish();
        }
        let family = match self.attachment.kind {
            AttachmentKind::Image => Family::Image,
            AttachmentKind::Audio => Family::Audio,
            AttachmentKind::Video => Family::Video,
            // A sender's file names and types are not trusted: the sniffed
            // family picks the row.
            AttachmentKind::File | AttachmentKind::Other => {
                match self.derive(Artifact::Metadata).await {
                    Part::Ready(derived) => match &*derived {
                        Derived::Metadata { mime, family } => {
                            built.view.detected_type = Some((*mime).to_owned());
                            *family
                        }
                        _ => Family::Other,
                    },
                    Part::Pending(retry) => {
                        built.pending(retry);
                        return built.finish();
                    }
                    Part::Failed(reason) => {
                        built.unavailable(reason);
                        return built.finish();
                    }
                }
            }
        };
        match plan(family, view) {
            Plan::Link => self.link(&mut built),
            Plan::Derive(artifact) => match self.derive(artifact).await {
                Part::Ready(derived) => {
                    if !built.take(&derived) {
                        built.unavailable(Reason::NotFound);
                    }
                }
                Part::Pending(retry) => built.pending(retry),
                Part::Failed(reason) => {
                    if artifact == Artifact::Visuals && reason == Reason::NotRetrievable {
                        built.notices.push(NO_FRAMES_NOTICE.into());
                    }
                    built.unavailable(reason);
                }
            },
            Plan::VideoAuto => {
                let (frames, transcript) = tokio::join!(
                    self.derive(Artifact::Visuals),
                    self.derive(Artifact::Transcript)
                );
                built.video(frames, transcript);
            }
            Plan::Unsupported(notice) => {
                built.notices.extend(notice.map(str::to_owned));
                built.unavailable(Reason::UnsupportedFormat);
            }
            Plan::MetadataOnly => built.notices.push(BINARY_NOTICE.into()),
        }
        built.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_bounded_by_index_and_page() {
        assert!(valid_request(0, 1) && valid_request(MAX_INDEX, MAX_PAGE));
        assert!(!valid_request(MAX_INDEX + 1, 1));
        assert!(!valid_request(0, 0) && !valid_request(0, MAX_PAGE + 1));
    }
}
