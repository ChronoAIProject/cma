//! The authority a media link re-validates on every download request.
//!
//! A link authorizes nothing the minting binding could no longer do itself:
//! the channel, binding, agent key, download permission, activity and source
//! are all checked again against their current state.
use super::{context, media_links::hex_bytes, ToolsState};
use crate::{
    channels::resolver::{ResolveError, ResolvedChannel},
    media::{service::MediaService, source},
    telemetry,
    turns::render_media,
};
use cmaeg_core::{
    activity::{Activity, Attachment},
    problem::{Problem, ProblemCode},
    seal::{media_link, EventRefPayload, MediaLinkPayload, MediaLinkRef, OpenError},
};
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// A refused download: the problem to answer, its bounded outcome label and,
/// for transient refusals, when to try again.
pub(crate) struct Refusal {
    pub(crate) problem: Problem,
    pub(crate) outcome: &'static str,
    pub(crate) retry_after: Option<u32>,
}

impl Refusal {
    pub(crate) fn new(outcome: &'static str, code: ProblemCode, status: u16, detail: &str) -> Self {
        Self {
            problem: Problem::new(code, status, detail),
            outcome,
            retry_after: None,
        }
    }

    /// The same refusal with a `Retry-After` in seconds.
    pub(crate) fn retry_after(mut self, seconds: u32) -> Self {
        self.retry_after = Some(seconds);
        self
    }

    fn revoked() -> Self {
        Self::new("revoked", ProblemCode::Forbidden, 403, "media_link_revoked")
    }
}

/// What a redeemed link still authorizes: the current channel, the event,
/// its verified activity and the named attachment.
pub(crate) struct Authorized {
    pub(crate) channel: Arc<ResolvedChannel>,
    pub(crate) event: EventRefPayload,
    pub(crate) activity: Activity,
    pub(crate) attachment: Attachment,
}

impl ToolsState {
    /// Open `token`, redeem it, and re-validate everything it claims against
    /// the current channel, binding, agent key, activity and source.
    pub(crate) async fn authorize_download(
        &self,
        media: &MediaService,
        token: &MediaLinkRef,
    ) -> Result<Authorized, Refusal> {
        let link = media_link::open(&self.keys, token, now()).map_err(|error| match error {
            OpenError::Expired => Refusal::new(
                "expired",
                ProblemCode::MediaUnavailable,
                410,
                "media_link_expired",
            ),
            _ => Refusal::new(
                "invalid",
                ProblemCode::InvalidRequest,
                400,
                "media_link_invalid",
            ),
        })?;
        tracing::Span::current().record(
            "channel_hash",
            telemetry::channel_hash(link.event.channel_id.as_str()).as_str(),
        );
        // Redemption comes before every other check, so a refused request
        // also consumes the link.
        if !self.downloads.redeem(&link.jti) {
            return Err(Refusal::new(
                "used",
                ProblemCode::MediaUnavailable,
                410,
                "media_link_used",
            ));
        }
        let channel = match self.resolver.resolve(link.event.channel_id.as_str()).await {
            Ok(channel) => channel,
            // A deleted or unreadable channel no longer authorizes anything.
            Err(ResolveError::NotFound | ResolveError::Tampered | ResolveError::Invalid) => {
                return Err(Refusal::revoked())
            }
            Err(error) => {
                return Err(Refusal {
                    problem: error.problem(),
                    outcome: if error == ResolveError::RateLimited {
                        "rate_limited"
                    } else {
                        "unavailable"
                    },
                    retry_after: None,
                })
            }
        };
        if !still_permitted(media, &channel, &link) {
            return Err(Refusal::revoked());
        }
        let unverified = || {
            Refusal::new(
                "unavailable",
                ProblemCode::MediaUnavailable,
                503,
                "event_context_unverified",
            )
        };
        let (activity, verified) = context::read_activity(self, &channel, &link.event)
            .await
            .map_err(|_| unverified())?;
        if !verified {
            return Err(unverified());
        }
        let attachment = render_media::attachments(&activity)
            .get(usize::from(link.index))
            .cloned()
            .ok_or_else(|| {
                Refusal::new(
                    "not_found",
                    ProblemCode::AttachmentNotFound,
                    404,
                    "attachment_not_found",
                )
            })?;
        if !source::configured(&channel, &activity, attachment.source.as_ref()) {
            return Err(Refusal::revoked());
        }
        Ok(Authorized {
            channel,
            event: link.event,
            activity,
            attachment,
        })
    }
}

/// The link's record, binding and agent key are still the channel's, and the
/// binding may still download: it is autonomous and its provider still
/// installs the gateway tools. Rotating the agent key changes its hash.
fn still_permitted(
    media: &MediaService,
    channel: &ResolvedChannel,
    link: &MediaLinkPayload,
) -> bool {
    let provider = &channel.definition.provider;
    let key_matches = hex_bytes::<32>(&link.agent_key_hash)
        .is_some_and(|hash| bool::from(channel.agent_key_hash.ct_eq(&hash)));
    channel.pointer.record_id == link.event.record_id
        && provider.slug == link.provider
        && provider.binding_id == link.binding_id
        && key_matches
        && channel
            .consumer(None)
            .is_some_and(|consumer| media.audience(&consumer).downloads)
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
