//! One HTTP exchange of a replaceable client transport.
//!
//! The NyxID relay and Telegram clients send through transports that answer
//! with these types, so a test can answer them in process. Under tokio's paused
//! clock a pending socket read lets the clock auto-advance and fire timeouts
//! spuriously; an in-process transport keeps every paused-clock offset exact.
use reqwest::{header::HeaderMap, Response, StatusCode};

/// No answer arrived: connecting or sending failed, or the request timed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    Unavailable,
}

/// Why an answer's body could not be read in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    /// The body exceeded the client's bound.
    Oversized,
    /// The body broke off or timed out while it was read.
    Lost,
}

/// An HTTP answer. Status and headers are always present, so a client maps the
/// status before it looks at the body, exactly as it did reading a live response.
#[derive(Debug, Clone)]
pub struct HttpReply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Result<Vec<u8>, BodyError>,
}

impl HttpReply {
    /// An answer with no headers and a complete body.
    pub fn new(status: StatusCode, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: Ok(body),
        }
    }
}

/// Read `response`'s body, refusing more than `max` bytes. A declared length
/// above the bound is refused before any chunk is read.
pub(crate) async fn read_bounded(mut response: Response, max: usize) -> Result<Vec<u8>, BodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > max as u64)
    {
        return Err(BodyError::Oversized);
    }
    let mut bytes = Vec::new();
    loop {
        // A chunk error can carry the request URL, which may embed a bot
        // token, so it is dropped here and never reaches a log.
        match response.chunk().await {
            Ok(Some(chunk)) if bytes.len() + chunk.len() <= max => bytes.extend_from_slice(&chunk),
            Ok(None) => return Ok(bytes),
            Ok(Some(_)) => return Err(BodyError::Oversized),
            Err(_) => return Err(BodyError::Lost),
        }
    }
}
