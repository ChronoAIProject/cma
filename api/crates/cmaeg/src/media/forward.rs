//! Response bodies that release their budget permits at a hard deadline.
//!
//! A reader that never polls must not hold global capacity until the 660 s
//! load-balancer timeout. Each body is fed by a task that owns the source and
//! every permit, sends through a one-slot channel, and gives up at the
//! deadline whether or not anyone is reading; the permits go with it.
use super::body::SourceStream;
use axum::body::Body;
use bytes::Bytes;
use futures_util::StreamExt;
use std::{
    any::Any,
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::Poll,
    time::Duration,
};
use tokio::{
    sync::mpsc,
    time::{timeout_at, Instant},
};

/// How long a tool may spend receiving one attachment body.
pub const TOOL_BODY_DEADLINE: Duration = Duration::from_secs(15);
/// How long a download link may spend sending one original.
pub const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(300);
const FORWARD_CHUNK: usize = 64 * 1024;

type Chunk = Result<Bytes, io::Error>;

/// Permits of any budget type, held by a body's feeding task and released
/// together when it ends.
#[derive(Default)]
pub struct PermitSet(Vec<Box<dyn Any + Send + Sync>>);

impl PermitSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push<T: Send + Sync + 'static>(&mut self, permit: T) {
        self.0.push(Box::new(permit));
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Send `bytes` in chunks of at most 64 KiB until `deadline`.
pub fn forward_bytes(bytes: Bytes, permits: PermitSet, deadline: Instant) -> Body {
    let (sender, receiver) = mpsc::channel::<Chunk>(1);
    let aborted = Arc::new(AtomicBool::new(false));
    let flag = aborted.clone();
    tokio::spawn(async move {
        let completed = timeout_at(deadline, send_bytes(&sender, &bytes))
            .await
            .unwrap_or(false);
        finish(completed, &flag, sender, permits);
    });
    body(receiver, aborted)
}

/// Forward `stream` in chunks of at most 64 KiB until `deadline`; an
/// upstream error aborts the body.
pub fn forward_stream(stream: SourceStream, permits: PermitSet, deadline: Instant) -> Body {
    let (sender, receiver) = mpsc::channel::<Chunk>(1);
    let aborted = Arc::new(AtomicBool::new(false));
    let flag = aborted.clone();
    tokio::spawn(async move {
        let completed = timeout_at(deadline, pump(&sender, stream))
            .await
            .unwrap_or(false);
        finish(completed, &flag, sender, permits);
    });
    body(receiver, aborted)
}

/// The abort flag is set and the permits released before the sender closes
/// the channel, so a reader that sees the channel end also sees whether the
/// body was cut short, and the budget is already free.
fn finish(completed: bool, aborted: &AtomicBool, sender: mpsc::Sender<Chunk>, permits: PermitSet) {
    if !completed {
        aborted.store(true, Ordering::Release);
        tracing::debug!(permits = permits.len(), "media response body aborted");
    }
    drop(permits);
    drop(sender);
}

/// True when every byte was handed to the channel; false when the reader
/// went away.
async fn send_bytes(sender: &mpsc::Sender<Chunk>, bytes: &Bytes) -> bool {
    let mut offset = 0;
    while offset < bytes.len() {
        let end = bytes.len().min(offset + FORWARD_CHUNK);
        if sender.send(Ok(bytes.slice(offset..end))).await.is_err() {
            return false;
        }
        offset = end;
    }
    true
}

/// True when the upstream ended cleanly and everything was handed on; false
/// on an upstream error or when the reader went away, noticed even while the
/// upstream is quiet.
async fn pump(sender: &mpsc::Sender<Chunk>, mut source: SourceStream) -> bool {
    loop {
        let next = tokio::select! {
            _ = sender.closed() => return false,
            next = source.stream.next() => next,
        };
        match next {
            None => return true,
            Some(Err(_)) => return false,
            Some(Ok(chunk)) => {
                if !send_bytes(sender, &chunk).await {
                    return false;
                }
            }
        }
    }
}

/// The body yields what the task sent. If the task gave up, the end of the
/// channel becomes one `TimedOut` error, so hyper aborts the response and a
/// resumed reader never mistakes a short body for a complete one.
fn body(mut receiver: mpsc::Receiver<Chunk>, aborted: Arc<AtomicBool>) -> Body {
    let mut ended = false;
    Body::from_stream(futures_util::stream::poll_fn(move |context| {
        if ended {
            return Poll::Ready(None);
        }
        match receiver.poll_recv(context) {
            Poll::Ready(Some(chunk)) => Poll::Ready(Some(chunk)),
            Poll::Ready(None) => {
                ended = true;
                if aborted.load(Ordering::Acquire) {
                    Poll::Ready(Some(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "media response body aborted",
                    ))))
                } else {
                    Poll::Ready(None)
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }))
}
