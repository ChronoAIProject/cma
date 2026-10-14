//! Byte-capped reads of an upstream response body.
//!
//! Sender-declared sizes are hints and are never used to size buffers beyond
//! the cap: the buffer is allocated once, at `min(Content-Length, cap)` or at
//! the cap, and never grows, so resident memory stays at most the bytes
//! received and a chunk that would not fit ends the read instead of
//! reallocating.
use super::FetchError;
use bytes::{Bytes, BytesMut};
use futures_util::{stream::BoxStream, StreamExt};
use reqwest::header::CONTENT_LENGTH;
use std::fmt;
use tokio::time::{timeout_at, Instant};

/// What a read does when the body is larger than its cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    /// Refuse the body as `TooLarge`.
    Reject,
    /// Keep the first `cap` bytes and mark the body truncated.
    Truncate,
}

/// A body read whole, or its first `cap` bytes under [`Overflow::Truncate`].
pub struct FetchedBody {
    pub bytes: Bytes,
    pub truncated: bool,
}

/// Content must never reach a log line, so only the size is shown.
impl fmt::Debug for FetchedBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FetchedBody")
            .field("bytes", &self.bytes.len())
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// A body forwarded chunk by chunk; the stream ends after its first error.
pub struct SourceStream {
    pub content_length: Option<u64>,
    pub stream: BoxStream<'static, Result<Bytes, FetchError>>,
}

impl fmt::Debug for SourceStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceStream")
            .field("content_length", &self.content_length)
            .finish_non_exhaustive()
    }
}

/// Read `response` into one exactly preallocated buffer of at most `cap`
/// bytes, every chunk under `deadline`.
///
/// A declared `Content-Length` above the cap is `TooLarge` before any body
/// byte is read under [`Overflow::Reject`]; under [`Overflow::Truncate`] the
/// first `cap` bytes are kept. A body longer than its declared length is
/// `Upstream`.
pub async fn read_capped(
    mut response: reqwest::Response,
    cap: u64,
    overflow: Overflow,
    deadline: Instant,
) -> Result<FetchedBody, FetchError> {
    let declared = declared_length(&response);
    if overflow == Overflow::Reject && declared.is_some_and(|length| length > cap) {
        return Err(FetchError::TooLarge);
    }
    let mut buffer = CappedBuffer::new(cap, declared, overflow)?;
    loop {
        let chunk = match timeout_at(deadline, response.chunk()).await {
            Err(_) => return Err(FetchError::TimedOut),
            Ok(Err(error)) => return Err(transport(error)),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(chunk))) => chunk,
        };
        if buffer.push(&chunk)? == Progress::Full {
            break;
        }
    }
    buffer.finish()
}

/// Stream `response` with a running byte count: a declared length above the
/// cap is `TooLarge` up front, and the first chunk that would cross the cap
/// yields `TooLarge` and ends the stream.
pub fn stream_capped(response: reqwest::Response, cap: u64) -> Result<SourceStream, FetchError> {
    let content_length = declared_length(&response);
    if content_length.is_some_and(|length| length > cap) {
        return Err(FetchError::TooLarge);
    }
    let stream = futures_util::stream::unfold(
        (response.bytes_stream().boxed(), 0_u64, false),
        move |(mut inner, seen, ended)| async move {
            if ended {
                return None;
            }
            match inner.next().await? {
                Err(error) => Some((Err(transport(error)), (inner, seen, true))),
                Ok(chunk) => {
                    let total = seen.saturating_add(chunk.len() as u64);
                    if total > cap {
                        Some((Err(FetchError::TooLarge), (inner, total, true)))
                    } else {
                        Some((Ok(chunk), (inner, total, false)))
                    }
                }
            }
        },
    );
    Ok(SourceStream {
        content_length,
        stream: stream.boxed(),
    })
}

/// The declared body length: the `Content-Length` header, or the transport's
/// exact size hint when the header is absent.
fn declared_length(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .or_else(|| response.content_length())
}

/// Classify a transport failure. The error is stripped of its URL first, so
/// nothing that inspects it later can expose a token-bearing path.
pub(crate) fn transport(error: reqwest::Error) -> FetchError {
    let error = error.without_url();
    if error.is_timeout() {
        FetchError::TimedOut
    } else {
        FetchError::Upstream
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Progress {
    More,
    Full,
}

/// The single allocation a body is read into.
struct CappedBuffer {
    buffer: BytesMut,
    /// The most bytes this body may hold: the declared length or the cap.
    limit: usize,
    /// The declared length when it is within the cap: the body must match it.
    exact: Option<usize>,
    /// The declared length exceeds the cap, so reaching the cap truncates.
    declared_over_cap: bool,
    overflow: Overflow,
    truncated: bool,
}

impl CappedBuffer {
    fn new(cap: u64, declared: Option<u64>, overflow: Overflow) -> Result<Self, FetchError> {
        let size = |value: u64| usize::try_from(value).map_err(|_| FetchError::TooLarge);
        let exact = declared
            .filter(|length| *length <= cap)
            .map(size)
            .transpose()?;
        let limit = exact.map_or_else(|| size(cap), Ok)?;
        Ok(Self {
            buffer: BytesMut::with_capacity(limit),
            limit,
            exact,
            declared_over_cap: declared.is_some_and(|length| length > cap),
            overflow,
            truncated: false,
        })
    }

    fn push(&mut self, chunk: &[u8]) -> Result<Progress, FetchError> {
        let room = self.limit - self.buffer.len();
        if chunk.len() > room {
            if self.exact.is_some() {
                return Err(FetchError::Upstream);
            }
            if self.overflow == Overflow::Reject {
                return Err(FetchError::TooLarge);
            }
            self.buffer.extend_from_slice(&chunk[..room]);
            self.truncated = true;
            return Ok(Progress::Full);
        }
        self.buffer.extend_from_slice(chunk);
        // A declared length above the cap already proves the rest is cut off.
        if self.declared_over_cap && self.buffer.len() == self.limit {
            self.truncated = true;
            return Ok(Progress::Full);
        }
        Ok(Progress::More)
    }

    fn finish(self) -> Result<FetchedBody, FetchError> {
        if self.exact.is_some_and(|length| length != self.buffer.len()) {
            return Err(FetchError::Upstream);
        }
        Ok(FetchedBody {
            bytes: self.buffer.freeze(),
            truncated: self.truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_reallocation() {
        for (cap, declared, overflow, chunks) in [
            (16, Some(10), Overflow::Reject, vec![4, 6]),
            (16, None, Overflow::Reject, vec![8, 8]),
            (16, None, Overflow::Truncate, vec![10, 10]),
            (16, Some(40), Overflow::Truncate, vec![12, 12]),
        ] {
            let mut buffer = CappedBuffer::new(cap, declared, overflow).unwrap();
            let pointer = buffer.buffer.as_ptr();
            let capacity = buffer.buffer.capacity();
            for length in chunks {
                if buffer.push(&vec![7; length]).unwrap() == Progress::Full {
                    break;
                }
            }
            assert_eq!(buffer.buffer.as_ptr(), pointer);
            assert_eq!(buffer.buffer.capacity(), capacity);
            let body = buffer.finish().unwrap();
            assert_eq!(body.bytes.as_ptr(), pointer);
        }
    }

    #[test]
    fn the_buffer_is_sized_by_the_smaller_of_declared_and_cap() {
        assert_eq!(
            CappedBuffer::new(100, Some(10), Overflow::Reject)
                .unwrap()
                .limit,
            10
        );
        assert_eq!(
            CappedBuffer::new(100, None, Overflow::Reject)
                .unwrap()
                .limit,
            100
        );
        assert_eq!(
            CappedBuffer::new(100, Some(1_000), Overflow::Truncate)
                .unwrap()
                .limit,
            100
        );
    }

    #[test]
    fn overflow_follows_the_declared_length_and_mode() {
        let mut exact = CappedBuffer::new(100, Some(4), Overflow::Truncate).unwrap();
        assert_eq!(exact.push(b"12345"), Err(FetchError::Upstream));
        let mut rejecting = CappedBuffer::new(4, None, Overflow::Reject).unwrap();
        assert_eq!(rejecting.push(b"12345"), Err(FetchError::TooLarge));
        let mut truncating = CappedBuffer::new(4, None, Overflow::Truncate).unwrap();
        assert_eq!(truncating.push(b"12345"), Ok(Progress::Full));
        let body = truncating.finish().unwrap();
        assert_eq!((&body.bytes[..], body.truncated), (&b"1234"[..], true));
        let short = CappedBuffer::new(100, Some(4), Overflow::Reject).unwrap();
        assert_eq!(short.finish().unwrap_err(), FetchError::Upstream);
    }

    #[test]
    fn debug_output_never_shows_content() {
        let body = FetchedBody {
            bytes: Bytes::from_static(b"private-bytes"),
            truncated: false,
        };
        assert_eq!(
            format!("{body:?}"),
            "FetchedBody { bytes: 13, truncated: false }"
        );
    }
}
