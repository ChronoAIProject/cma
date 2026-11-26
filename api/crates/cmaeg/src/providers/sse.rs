//! Bounded Server-Sent Events framing for provider response streams.

use std::{collections::VecDeque, fmt};

use futures_util::{stream, Stream, StreamExt};

pub const MAX_SSE_FRAME_BYTES: usize = 1024 * 1024;

/// One parsed SSE frame, before its JSON data is interpreted as a response event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
    pub retry: Option<u64>,
}

/// Framing failures are distinct from typed provider event failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseError {
    FrameTooLarge,
    InvalidUtf8,
    Stream(String),
    Truncated,
}

impl SseError {
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::FrameTooLarge => "frame_too_large",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::Stream(_) => "transport",
            Self::Truncated => "truncated",
        }
    }
}

impl fmt::Display for SseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooLarge => formatter.write_str("SSE frame exceeds the 1 MiB limit"),
            Self::InvalidUtf8 => formatter.write_str("SSE stream contains invalid UTF-8"),
            Self::Stream(detail) => write!(formatter, "SSE stream failed: {detail}"),
            Self::Truncated => {
                formatter.write_str("SSE stream ended without a terminal response event")
            }
        }
    }
}

impl std::error::Error for SseError {}

/// Parse a byte stream into bounded SSE frames.
pub fn parse_sse<S, B, E>(stream: S) -> impl Stream<Item = Result<SseFrame, SseError>>
where
    S: Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: fmt::Display,
{
    stream::unfold(
        ParseState {
            stream,
            decoder: Decoder::default(),
            pending: VecDeque::new(),
            ended: false,
        },
        |mut state| async move {
            loop {
                if let Some(item) = state.pending.pop_front() {
                    return Some((item, state));
                }
                if state.ended {
                    return None;
                }
                match state.stream.next().await {
                    Some(Ok(chunk)) => {
                        let mut frames = Vec::new();
                        let result = state.decoder.feed(chunk.as_ref(), &mut frames);
                        // A transport chunk can contain complete frames before a framing error.
                        state.pending.extend(frames.into_iter().map(Ok));
                        if let Err(error) = result {
                            state.pending.push_back(Err(error));
                            state.ended = true;
                        }
                    }
                    Some(Err(error)) => {
                        state
                            .pending
                            .push_back(Err(SseError::Stream(error.to_string())));
                        state.ended = true;
                    }
                    None => {
                        state.ended = true;
                        match state.decoder.finish() {
                            Ok(frames) => state.pending.extend(frames.into_iter().map(Ok)),
                            Err(error) => state.pending.push_back(Err(error)),
                        }
                    }
                }
            }
        },
    )
}

struct ParseState<S> {
    stream: S,
    decoder: Decoder,
    pending: VecDeque<Result<SseFrame, SseError>>,
    ended: bool,
}

#[derive(Default)]
struct Decoder {
    bytes: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
    id: Option<String>,
    retry: Option<u64>,
    frame_bytes: usize,
    saw_terminal: bool,
}

impl Decoder {
    fn feed(&mut self, bytes: &[u8], frames: &mut Vec<SseFrame>) -> Result<(), SseError> {
        for byte in bytes {
            self.bytes.push(*byte);
            if self.bytes.len() > MAX_SSE_FRAME_BYTES {
                return Err(SseError::FrameTooLarge);
            }
            if *byte == b'\n' {
                let line = std::mem::take(&mut self.bytes);
                let line = line.strip_suffix(b"\n").unwrap_or(&line);
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                self.process_line(line, frames)?;
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<Vec<SseFrame>, SseError> {
        let mut frames = Vec::new();
        if !self.bytes.is_empty() {
            let line = std::mem::take(&mut self.bytes);
            self.process_line(&line, &mut frames)?;
        }
        if self.event.is_some()
            || !self.data.is_empty()
            || self.id.is_some()
            || self.retry.is_some()
        {
            return Err(SseError::Truncated);
        }
        if self.saw_terminal {
            Ok(frames)
        } else {
            Err(SseError::Truncated)
        }
    }

    fn process_line(&mut self, line: &[u8], frames: &mut Vec<SseFrame>) -> Result<(), SseError> {
        self.frame_bytes = self.frame_bytes.saturating_add(line.len());
        if self.frame_bytes > MAX_SSE_FRAME_BYTES {
            return Err(SseError::FrameTooLarge);
        }
        if line.is_empty() {
            if self.event.is_some()
                || !self.data.is_empty()
                || self.id.is_some()
                || self.retry.is_some()
            {
                let frame = SseFrame {
                    event: self.event.take(),
                    data: self.data.drain(..).collect::<Vec<_>>().join("\n"),
                    id: self.id.take(),
                    retry: self.retry.take(),
                };
                self.saw_terminal |= is_terminal(&frame);
                frames.push(frame);
            }
            self.frame_bytes = 0;
            return Ok(());
        }
        if line[0] == b':' {
            return Ok(());
        }
        let text = std::str::from_utf8(line).map_err(|_| SseError::InvalidUtf8)?;
        let (field, value) = text
            .split_once(':')
            .map(|(field, value)| (field, value.strip_prefix(' ').unwrap_or(value)))
            .unwrap_or((text, ""));
        match field {
            "event" => self.event = Some(value.to_owned()),
            "data" => self.data.push(value.to_owned()),
            "id" => self.id = Some(value.to_owned()),
            "retry" => self.retry = value.parse().ok(),
            _ => {}
        }
        Ok(())
    }
}

fn is_terminal(frame: &SseFrame) -> bool {
    if frame.event.as_deref().is_some_and(|event| {
        matches!(
            event,
            "response.completed" | "response.failed" | "response.incomplete"
        )
    }) {
        return true;
    }
    serde_json::from_str::<serde_json::Value>(&frame.data)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|kind| {
            matches!(
                kind.as_str(),
                "response.completed" | "response.failed" | "response.incomplete"
            )
        })
}

#[cfg(test)]
mod tests {
    use futures_util::{stream, StreamExt};

    use super::*;

    #[tokio::test]
    async fn handles_split_utf8_crlf_comments_and_multiline_data() {
        let bytes = "event: response.created\r\ndata: {\"id\":\"r\"}\r\n\r\n:event heartbeat\n\ndata: {\"type\":\"response.completed\",\"text\":\"hé\"}\n\n".as_bytes();
        let chunks = bytes
            .chunks(3)
            .map(|chunk| Ok::<_, std::convert::Infallible>(chunk.to_vec()))
            .collect::<Vec<_>>();
        let frames = parse_sse(stream::iter(chunks)).collect::<Vec<_>>().await;
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[0].as_ref().unwrap().event.as_deref(),
            Some("response.created")
        );
        assert_eq!(
            frames[1].as_ref().unwrap().data,
            r#"{"type":"response.completed","text":"hé"}"#
        );
    }

    #[tokio::test]
    async fn reports_truncated_stream_and_frame_limit() {
        let frames = parse_sse(stream::iter(vec![Ok::<_, std::convert::Infallible>(
            b"event: response.created\ndata: {}\n\n".to_vec(),
        )]))
        .collect::<Vec<_>>()
        .await;
        assert_eq!(frames.last(), Some(&Err(SseError::Truncated)));
        let frames = parse_sse(stream::iter(vec![Ok::<_, std::convert::Infallible>(
            vec![b'x'; MAX_SSE_FRAME_BYTES + 1],
        )]))
        .collect::<Vec<_>>()
        .await;
        assert_eq!(frames, vec![Err(SseError::FrameTooLarge)]);
    }

    #[tokio::test]
    async fn accepts_one_large_chunk_containing_bounded_frames() {
        let mut chunk = Vec::new();
        for _ in 0..2 {
            chunk.extend_from_slice(b"data: ");
            chunk.extend(std::iter::repeat_n(b'x', MAX_SSE_FRAME_BYTES / 2));
            chunk.extend_from_slice(b"\n\n");
        }
        chunk.extend_from_slice(
            b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r\"}}\n\n",
        );
        let frames = parse_sse(stream::iter(vec![Ok::<_, std::convert::Infallible>(chunk)]))
            .collect::<Vec<_>>()
            .await;
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(Result::is_ok));
    }

    #[tokio::test]
    async fn reports_a_malformed_suffix_after_completed_frames_independent_of_chunking() {
        let accepted = b"event: response.created\ndata: {}\n\n".to_vec();
        let completed = b"event: response.completed\ndata: {}\n\n".to_vec();
        let bad = b"data: \xff\n\n".to_vec();
        let expected = vec![
            Ok(SseFrame {
                event: Some("response.created".into()),
                data: "{}".into(),
                id: None,
                retry: None,
            }),
            Ok(SseFrame {
                event: Some("response.completed".into()),
                data: "{}".into(),
                id: None,
                retry: None,
            }),
            Err(SseError::InvalidUtf8),
        ];
        for chunks in [
            vec![accepted.clone(), completed.clone(), bad.clone()],
            vec![[accepted, completed, bad].concat()],
        ] {
            let frames = parse_sse(stream::iter(
                chunks.into_iter().map(Ok::<_, std::convert::Infallible>),
            ))
            .collect::<Vec<_>>()
            .await;
            assert_eq!(frames, expected);
        }
    }
}
