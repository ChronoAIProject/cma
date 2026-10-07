//! In-memory media sources that count their fetches, and verified targets
//! built around them.
use super::support::{activity, event, resolved};
use bytes::Bytes;
use cmaeg::media::{
    body::{FetchedBody, Overflow},
    source::{FetchRequest, MediaSource, SourceCredential, SourceRegistry, SourceStream},
    target::MediaTarget,
    FetchError,
};
use cmaeg_core::{
    activity::{ActivityKind, Attachment, AttachmentKind, AttachmentSource, SourceKind},
    channel::Platform,
    seal::{event_ref::activity_digest, ReplyAddress},
};
use futures_util::future::BoxFuture;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

/// What one fetch asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    pub cap: u64,
    pub overflow: Overflow,
    pub credential: SourceCredential,
}

/// Serves bodies by locator, after an optional delay, and records every
/// fetch. An unknown locator is `not_found`.
#[derive(Default)]
pub struct FakeSource {
    bodies: Mutex<HashMap<String, Result<Bytes, FetchError>>>,
    delay: Mutex<Duration>,
    calls: AtomicUsize,
    seen: Mutex<Vec<Seen>>,
}

/// The identifying string of a locator.
pub fn locator_key(locator: &AttachmentSource) -> String {
    match locator {
        AttachmentSource::NyxidRelay { index } => format!("relay:{index}"),
        AttachmentSource::Telegram { file_unique_id, .. } => format!("telegram:{file_unique_id}"),
        AttachmentSource::Url { url } => url.clone(),
        AttachmentSource::Inline { pointer } => format!("inline:{pointer}"),
    }
}

impl FakeSource {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// Serve `result` for `locator`.
    pub fn serve(&self, locator: &AttachmentSource, result: Result<Vec<u8>, FetchError>) {
        self.bodies
            .lock()
            .unwrap()
            .insert(locator_key(locator), result.map(Bytes::from));
    }

    /// Delay every later fetch by `delay`.
    pub fn delay(&self, delay: Duration) {
        *self.delay.lock().unwrap() = delay;
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// A registry with this source behind every locator type.
    pub fn registry(self: &Arc<Self>) -> Arc<SourceRegistry> {
        Arc::new(SourceRegistry::new(
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
        ))
    }
}

impl MediaSource for FakeSource {
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchedBody, FetchError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(Seen {
            cap: request.cap,
            overflow: request.overflow,
            credential: request.credential,
        });
        let delay = *self.delay.lock().unwrap();
        let body = request.locator.and_then(|locator| {
            self.bodies
                .lock()
                .unwrap()
                .get(&locator_key(locator))
                .cloned()
        });
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            let bytes = body.unwrap_or(Err(FetchError::NotFound))?;
            let cap = usize::try_from(request.cap).unwrap();
            if bytes.len() <= cap {
                return Ok(FetchedBody {
                    bytes,
                    truncated: false,
                });
            }
            match request.overflow {
                Overflow::Reject => Err(FetchError::TooLarge),
                Overflow::Truncate => Ok(FetchedBody {
                    bytes: bytes.slice(..cap),
                    truncated: true,
                }),
            }
        })
    }

    fn open<'a>(
        &'a self,
        _request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<SourceStream, FetchError>> {
        Box::pin(async { Err(FetchError::Rejected) })
    }
}

pub fn url(name: &str) -> AttachmentSource {
    AttachmentSource::Url {
        url: format!("https://files.example/{name}"),
    }
}

pub fn telegram_file(unique: &str) -> AttachmentSource {
    AttachmentSource::Telegram {
        file_id: format!("file-{unique}"),
        file_unique_id: unique.into(),
    }
}

/// An attachment with only the facts a test sets.
pub fn attachment(kind: AttachmentKind, source: Option<AttachmentSource>) -> Attachment {
    Attachment {
        reference: "item-0".into(),
        kind,
        filename: None,
        mime_type: None,
        size_bytes: None,
        source,
        variant: None,
        duration_seconds: None,
        width: None,
        height: None,
        thumbnail: None,
        emoji: None,
    }
}

pub fn message(attachments: Vec<Attachment>) -> ActivityKind {
    ActivityKind::Message {
        text: "see attached".into(),
        attachments,
        mentions_bot: true,
        reply_to: None,
        album_id: None,
    }
}

pub fn telegram_source() -> SourceKind {
    SourceKind::Telegram {
        platform: Platform::Telegram,
        bot_id: "77".into(),
    }
}

/// A verified target on the fixture channel for an activity of `source` and
/// `kind`, sealed with its digest.
pub fn target(source: SourceKind, kind: ActivityKind) -> MediaTarget {
    let activity = activity(source, Some(kind));
    let mut payload = event(ReplyAddress::None);
    payload.activity_sha256 = Some(activity_digest(&activity));
    MediaTarget::new(Arc::new(resolved(|_| {})), payload, Arc::new(activity)).unwrap()
}

/// A verified custom message carrying `attachments`.
pub fn custom_target(attachments: Vec<Attachment>) -> MediaTarget {
    target(SourceKind::Custom, message(attachments))
}
