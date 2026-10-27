//! Where an attachment's bytes come from, behind one injected registry.
//!
//! Each locator type has one [`MediaSource`]. The registry dispatches on the
//! locator, refuses a locator that does not match the activity's source, and
//! records every call. Sources receive the *current* resolved channel, so a
//! credential or allowlist removed since the event arrived is no longer used.
//! No source puts a URL, token, file identifier, file name or message ID in an
//! error, a log line or a span.
pub mod discord;
pub mod inline;
pub(crate) mod lark;
pub mod nyxid;
pub mod telegram;
pub mod url;

pub use crate::media::body::SourceStream;

use super::{
    body::{FetchedBody, Overflow},
    FetchError,
};
use crate::{
    channels::resolver::ResolvedChannel,
    telemetry::{self, names},
};
use cmaeg_core::{
    activity::{Activity, AttachmentSource, SourceKind},
    seal::EventRefPayload,
};
use futures_util::{future::BoxFuture, StreamExt};
use std::{sync::Arc, time::Instant as StdInstant};
use tokio::time::Instant;
use tracing::{field::Empty, Instrument};

/// Which relay credential a NyxID fetch starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCredential {
    /// The event's reply token while it is unexpired, falling back once to
    /// the channel key when NyxID refuses it.
    ReplyToken,
    /// Only the channel key.
    ChannelKey,
}

/// Whether `channel`'s current definition still configures the source that
/// delivered `locator` in `activity`: the relay route, the native bot and its
/// token, or the custom origin. A download link re-checks this on every
/// request, so removing any of them revokes it. Inline bytes travel inside
/// the verified activity and need no source.
pub fn configured(
    channel: &ResolvedChannel,
    activity: &Activity,
    locator: Option<&AttachmentSource>,
) -> bool {
    match locator {
        Some(AttachmentSource::Discord { .. }) => discord::configured(channel, &activity.source),
        Some(locator @ AttachmentSource::LarkResource { .. }) => {
            lark::configured(channel, &activity.source, locator)
        }
        Some(AttachmentSource::NyxidRelay { .. }) => nyxid::configured(channel, &activity.source),
        Some(AttachmentSource::Telegram { .. }) => {
            telegram::configured_token(channel, &activity.source).is_some()
        }
        Some(AttachmentSource::Url { url }) => url::configured(channel, url),
        Some(AttachmentSource::Inline { .. }) | None => true,
    }
}

/// One retrieval: the attachment's locator with the authority to resolve it.
#[derive(Clone, Copy)]
pub struct FetchRequest<'a> {
    /// The current resolved record: credentials and sources.
    pub channel: &'a ResolvedChannel,
    /// The opened event_ref: reply address and expiry.
    pub event: &'a EventRefPayload,
    /// The verified activity: inline reads and the source kind.
    pub activity: &'a Activity,
    /// `attachment.source`, or `attachment.thumbnail`.
    pub locator: Option<&'a AttachmentSource>,
    /// `attachment.size_bytes`: a hint, never a buffer size.
    pub declared_size: Option<u64>,
    /// Relay only.
    pub credential: SourceCredential,
    pub cap: u64,
    pub overflow: Overflow,
    pub deadline: Instant,
}

/// Resolves one locator type.
pub trait MediaSource: Send + Sync {
    /// Read the whole body, at most `request.cap` bytes.
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchedBody, FetchError>>;
    /// Open the body as a capped stream.
    fn open<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<SourceStream, FetchError>>;
}

/// The four sources, each injected so tests and later callers can swap an
/// implementation without touching anything that fetches.
#[derive(Clone)]
pub struct SourceRegistry {
    discord: Option<Arc<dyn MediaSource>>,
    nyxid: Arc<dyn MediaSource>,
    telegram: Arc<dyn MediaSource>,
    url: Arc<dyn MediaSource>,
    inline: Arc<dyn MediaSource>,
}

impl SourceRegistry {
    pub fn new(
        nyxid: Arc<dyn MediaSource>,
        telegram: Arc<dyn MediaSource>,
        url: Arc<dyn MediaSource>,
        inline: Arc<dyn MediaSource>,
    ) -> Self {
        Self {
            discord: None,
            nyxid,
            telegram,
            url,
            inline,
        }
    }

    pub fn with_discord(mut self, source: Arc<dyn MediaSource>) -> Self {
        self.discord = Some(source);
        self
    }

    /// Read the attachment's whole body through the source its locator names.
    pub async fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchedBody, FetchError> {
        let (label, source) = self.route(&request)?;
        let call = Call::start(label, &request);
        let result = source.fetch(request).instrument(call.span.clone()).await;
        call.finish(
            result
                .as_ref()
                .map(|body| body.bytes.len() as u64)
                .map_err(|error| *error),
        );
        result
    }

    /// Open the attachment's body as a stream through the source its locator
    /// names. The duration covers opening; bytes are counted as they pass.
    pub async fn open(&self, request: FetchRequest<'_>) -> Result<SourceStream, FetchError> {
        let (label, source) = self.route(&request)?;
        let call = Call::start(label, &request);
        let result = source.open(request).instrument(call.span.clone()).await;
        call.finish(result.as_ref().map(|_| 0).map_err(|error| *error));
        result.map(|opened| SourceStream {
            content_length: opened.content_length,
            stream: opened
                .stream
                .inspect(move |chunk| {
                    if let Ok(chunk) = chunk {
                        metrics::counter!(names::MEDIA_FETCH_BYTES, names::SOURCE => label)
                            .increment(chunk.len() as u64);
                    }
                })
                .boxed(),
        })
    }

    /// The source for the request's locator. Without a locator there is no
    /// source to attribute the call to, so it is refused unrecorded. A locator
    /// of the wrong type for the activity's source is refused (and recorded)
    /// as defence in depth, since ingress validation already pairs them.
    fn route(
        &self,
        request: &FetchRequest<'_>,
    ) -> Result<(&'static str, &Arc<dyn MediaSource>), FetchError> {
        let Some(locator) = request.locator else {
            tracing::debug!("media fetch requested for an attachment without a locator");
            return Err(FetchError::NotRetrievable);
        };
        let (label, source, matches) = match locator {
            AttachmentSource::Discord { .. } => (
                "discord",
                self.discord.as_ref().ok_or(FetchError::NotRetrievable)?,
                matches!(request.activity.source, SourceKind::Discord { .. }),
            ),
            AttachmentSource::LarkResource { .. } => (
                "lark_resource",
                &self.nyxid,
                lark::configured(request.channel, &request.activity.source, locator),
            ),
            AttachmentSource::NyxidRelay { .. } => (
                "nyxid",
                &self.nyxid,
                matches!(request.activity.source, SourceKind::NyxidRelay { .. }),
            ),
            AttachmentSource::Telegram { .. } => (
                "telegram",
                &self.telegram,
                matches!(request.activity.source, SourceKind::Telegram { .. }),
            ),
            AttachmentSource::Url { .. } => (
                "url",
                &self.url,
                request.activity.source == SourceKind::Custom,
            ),
            AttachmentSource::Inline { .. } => (
                "inline",
                &self.inline,
                request.activity.source == SourceKind::Custom,
            ),
        };
        if !matches {
            Call::start(label, request).finish(Err(FetchError::NotRetrievable));
            return Err(FetchError::NotRetrievable);
        }
        Ok((label, source))
    }
}

/// The metrics and span of one registry call. The span names only the
/// source, the outcome, the byte count and the channel's hash.
struct Call {
    label: &'static str,
    span: tracing::Span,
    started: StdInstant,
}

impl Call {
    fn start(label: &'static str, request: &FetchRequest<'_>) -> Self {
        let span = tracing::info_span!(
            "cmaeg.media.fetch",
            source = label,
            channel = %telemetry::channel_hash(request.channel.id.as_str()),
            outcome = Empty,
            bytes = Empty,
        );
        Self {
            label,
            span,
            started: StdInstant::now(),
        }
    }

    fn finish(self, result: Result<u64, FetchError>) {
        let outcome = result.err().map_or("ok", |error| error.as_str());
        let bytes = result.unwrap_or(0);
        self.span.record("outcome", outcome);
        self.span.record("bytes", bytes);
        metrics::histogram!(
            names::MEDIA_FETCH_SECONDS,
            names::SOURCE => self.label,
            names::OUTCOME => outcome
        )
        .record(self.started.elapsed().as_secs_f64());
        metrics::counter!(names::MEDIA_FETCH_BYTES, names::SOURCE => self.label).increment(bytes);
        self.span.in_scope(|| {
            if result.is_err() {
                tracing::info!(outcome, "media fetch failed");
            } else {
                tracing::debug!(outcome, bytes, "media fetch finished");
            }
        });
    }
}
