//! Bounded media work between a turn's admission and its provider start.
//!
//! Providers take one `input_text` per turn, and agents without the gateway's
//! tools (existing-agent bindings, providers such as `nyxbot`) can read an
//! attachment only through that text. So transcripts of short audio and
//! video, and excerpts of small text files, are derived eagerly and inlined
//! within a per-turn budget. What is not ready in time is rendered as pending
//! and keeps running into the cache, and failures render their bounded
//! reason: nothing is ever fabricated. Images and visuals are only warmed in
//! the background, never awaited, and only for agents that hold the tools to
//! read them.
use super::{
    derive::{Artifact, DeriveError, Derived},
    jobs::JobOutcome,
    service::MediaService,
    sniff::text_like_hint,
    target::MediaTarget,
    text::EXCERPT_READ_LIMIT,
    visual::VisualKind,
};
use crate::{
    ingress::launch::LaunchRequest,
    telemetry::{self, names},
    turns::render_media::{
        self, Audience, InlineKind, InlineMedia, InlineNotice, ItemNote, RenderMedia,
    },
};
use cmaeg_core::activity::{Attachment, AttachmentKind, MediaVariant};
use futures_util::future::join_all;
use std::time::{Duration, Instant as StdInstant};
use tokio::time::Instant;
use tracing::{field::Empty, Instrument};

/// Longest declared video whose audio track is transcribed eagerly; video
/// notes are always short and qualify without a declared duration.
pub const VIDEO_EAGER_MAX_SECONDS: u32 = 60;
/// The reason every eager item renders when the turn's own event_ref does not
/// authorize reads; it is one of the bounded attachment-view reason codes.
const NOT_RETRIEVABLE: &str = "not_retrievable";

/// How much eager work a turn may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrepareConfig {
    /// `CMAEG_MEDIA_PREPARE_BUDGET_MS`.
    pub budget: Duration,
    /// `CMAEG_STT_EAGER_MAX_SECONDS`; 0 disables eager audio transcripts.
    pub eager_audio_max_seconds: u32,
}

impl Default for PrepareConfig {
    fn default() -> Self {
        Self {
            budget: Duration::from_secs(15),
            eager_audio_max_seconds: 300,
        }
    }
}

/// The media a turn renders, and whether any eager item ran for it.
pub struct Prepared {
    pub media: RenderMedia,
    pub eager: bool,
}

/// Prepare `turn`'s attachments for `audience`, waiting for eager items until
/// `deadline` at the latest. `now` is the Unix time the event_ref is checked
/// against.
pub async fn prepare(
    service: &MediaService,
    turn: &LaunchRequest,
    audience: Audience,
    config: &PrepareConfig,
    nonce: Option<String>,
    deadline: Instant,
    now: i64,
) -> Prepared {
    let attachments = render_media::attachments(&turn.activity);
    if attachments.is_empty() {
        return Prepared {
            media: RenderMedia::empty(nonce),
            eager: false,
        };
    }
    // Without a fence marker no sender text may reach the prompt, so there is
    // nothing to prepare; the RNG failure was already logged where it happened.
    if nonce.is_none() {
        return Prepared {
            media: RenderMedia::empty(None),
            eager: false,
        };
    }
    let eager: Vec<(usize, Artifact)> = attachments
        .iter()
        .enumerate()
        .filter_map(|(index, item)| eager_artifact(item, config).map(|artifact| (index, artifact)))
        .collect();
    let warm: Vec<(usize, Artifact)> = if audience.tools {
        attachments
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                warm_artifact(item, service).map(|artifact| (index, artifact))
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut media = RenderMedia::empty(nonce);
    if eager.is_empty() && warm.is_empty() {
        return Prepared {
            media,
            eager: false,
        };
    }
    let channel = telemetry::channel_hash(turn.channel.id.as_str());
    let target = match service.target_for_turn(turn, now) {
        Ok(target) => target,
        Err(error) => {
            tracing::warn!(
                channel = %channel,
                error = error.as_str(),
                "media target of a turn is unverifiable; eager items are not retrievable"
            );
            media.notes = eager
                .iter()
                .map(|(index, _)| (*index, ItemNote::Unavailable(NOT_RETRIEVABLE)))
                .collect();
            return Prepared {
                media,
                eager: false,
            };
        }
    };
    let ran = !eager.is_empty();
    if ran {
        let span = tracing::info_span!(
            "cmaeg.media.prepare",
            items = attachments.len(),
            eager = eager.len(),
            outcome = Empty,
            channel = %channel,
        );
        let outcome = run_eager(service, &target, &eager, deadline, &mut media)
            .instrument(span.clone())
            .await;
        span.record("outcome", outcome);
    }
    // After the eager items, so that they are first in line for job permits;
    // the agent cannot ask for a warmed item before its turn starts anyway.
    for (index, artifact) in warm {
        service.warm(&target, index, artifact);
    }
    Prepared { media, eager: ran }
}

/// Derive every eager item concurrently and fold the outcomes into `media`,
/// returning the turn's preparation outcome.
async fn run_eager(
    service: &MediaService,
    target: &MediaTarget,
    eager: &[(usize, Artifact)],
    deadline: Instant,
    media: &mut RenderMedia,
) -> &'static str {
    let started = StdInstant::now();
    let wait = deadline.saturating_duration_since(Instant::now());
    let outcomes = join_all(
        eager
            .iter()
            .map(|(index, artifact)| service.derive(target, *index, *artifact, wait)),
    )
    .await;
    let (mut ready, mut pending) = (0, 0);
    for ((index, _), outcome) in eager.iter().zip(outcomes) {
        match outcome {
            JobOutcome::Ready(derived) => match inline(*index, &derived) {
                Some(item) => {
                    ready += 1;
                    media.inline.push(item);
                }
                None => {
                    tracing::error!(index, "eager derivation returned an unexpected artifact");
                    media.notes.push((
                        *index,
                        ItemNote::Unavailable(DeriveError::TransformFailed.as_str()),
                    ));
                }
            },
            JobOutcome::Pending { .. } => {
                pending += 1;
                media.notes.push((*index, ItemNote::Pending));
            }
            JobOutcome::Failed(error) => {
                media
                    .notes
                    .push((*index, ItemNote::Unavailable(error.as_str())));
            }
        }
    }
    let outcome = if ready == eager.len() {
        "ready"
    } else if pending > 0 {
        "pending"
    } else {
        "unavailable"
    };
    metrics::histogram!(names::MEDIA_PREPARE, names::OUTCOME => outcome)
        .record(started.elapsed().as_secs_f64());
    tracing::debug!(
        items = eager.len(),
        ready,
        pending,
        outcome,
        "eager media items prepared"
    );
    outcome
}

/// The inline text of a ready eager artifact: a transcript's first page or an
/// excerpt. The renderer enforces the per-item and per-turn byte caps.
fn inline(index: usize, derived: &Derived) -> Option<InlineMedia> {
    match derived {
        Derived::Transcript {
            document,
            truncated_at_seconds,
            ..
        } => Some(InlineMedia {
            index,
            kind: InlineKind::Transcript,
            text: document.pages.first().cloned().unwrap_or_default(),
            truncated: document.pages.len() > 1 || document.truncated,
            notice: truncated_at_seconds.map(|seconds| InlineNotice::AudioTruncated { seconds }),
        }),
        Derived::Excerpt { text, truncated } => Some(InlineMedia {
            index,
            kind: InlineKind::Excerpt,
            text: text.clone(),
            truncated: *truncated,
            notice: None,
        }),
        _ => None,
    }
}

/// What, if anything, is derived for `attachment` before its turn starts.
///
/// Declared facts decide eligibility only: a sender inflating a duration can
/// only make an item ineligible, and the transcriber, upload cap and spend
/// budgets are still enforced by the service, surfacing as reasons.
fn eager_artifact(attachment: &Attachment, config: &PrepareConfig) -> Option<Artifact> {
    // Without a source the descriptor already says it is not retrievable.
    attachment.source.as_ref()?;
    let eligible = match attachment.kind {
        AttachmentKind::Audio => {
            config.eager_audio_max_seconds > 0
                && attachment
                    .duration_seconds
                    .is_none_or(|seconds| seconds <= config.eager_audio_max_seconds)
        }
        AttachmentKind::Video => match attachment.variant {
            Some(MediaVariant::Animation | MediaVariant::Sticker) => false,
            Some(MediaVariant::VideoNote) => true,
            _ => attachment
                .duration_seconds
                .is_some_and(|seconds| seconds <= VIDEO_EAGER_MAX_SECONDS),
        },
        AttachmentKind::File | AttachmentKind::Other => {
            return (text_like_hint(
                attachment.mime_type.as_deref(),
                attachment.filename.as_deref(),
            ) && attachment
                .size_bytes
                .is_none_or(|size| size <= EXCERPT_READ_LIMIT))
            .then_some(Artifact::Excerpt);
        }
        AttachmentKind::Image => false,
    };
    eligible.then_some(Artifact::Transcript)
}

/// What is warmed for a tool-bearing agent: a normalized image, or a video's
/// visuals when there is a thumbnail or keyframes can be extracted.
fn warm_artifact(attachment: &Attachment, service: &MediaService) -> Option<Artifact> {
    match attachment.kind {
        AttachmentKind::Image => attachment.source.is_some().then_some(Artifact::Image),
        AttachmentKind::Video => {
            let keyframes = attachment.source.is_some()
                && service.capabilities().visuals == VisualKind::Keyframes;
            (attachment.thumbnail.is_some() || keyframes).then_some(Artifact::Visuals)
        }
        _ => None,
    }
}
