//! A fake OpenAI-compatible speech-to-text endpoint that records every
//! request and answers from a script, and the transcriber configured for it.
use super::support::{isolated, serve};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use cmaeg::{
    media::transcribe::{openai::OpenAiCompatibleTranscriber, AudioContainer, AudioInput},
    ConfigError, SttConfig,
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::watch;

pub(super) const KEY: &str = "stt-platform-key-must-stay-private";
pub(super) const PATH: &str = "/v1/audio/transcriptions";
pub(super) const SPOKEN: &str = "the quick brown fox transcript";

/// One scripted answer.
#[derive(Clone)]
pub(super) struct Reply {
    pub status: u16,
    pub body: String,
    pub delay: Duration,
}

impl Reply {
    pub(super) fn ok(body: &str) -> Self {
        Self::status(200, body)
    }

    pub(super) fn status(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }
}

/// What the endpoint received.
#[derive(Clone, Debug)]
pub(super) struct Received {
    pub authorization: Option<String>,
    pub content_type: String,
    pub content_length: Option<u64>,
    pub body: Vec<u8>,
}

/// One part of a received multipart body.
#[derive(Debug)]
pub(super) struct FormPart {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

impl Received {
    /// The multipart parts, split on the boundary the content type names.
    pub(super) fn parts(&self) -> Vec<FormPart> {
        let boundary = self
            .content_type
            .strip_prefix("multipart/form-data; boundary=")
            .expect("a multipart content type");
        let delimiter = format!("--{boundary}");
        let mut parts = Vec::new();
        let mut rest = &self.body[..];
        while let Some(start) = find(rest, delimiter.as_bytes()) {
            rest = &rest[start + delimiter.len()..];
            if rest.starts_with(b"--") {
                break;
            }
            let rest_part = rest.strip_prefix(b"\r\n").expect("a part line break");
            let end = find(rest_part, delimiter.as_bytes()).expect("a closing delimiter");
            let part = &rest_part[..end - 2];
            let split = find(part, b"\r\n\r\n").expect("part headers");
            let headers = String::from_utf8(part[..split].to_vec()).unwrap();
            parts.push(FormPart {
                name: attribute(&headers, "name").expect("a part name"),
                filename: attribute(&headers, "filename"),
                content_type: headers.lines().find_map(|line| {
                    line.strip_prefix("Content-Type: ")
                        .or_else(|| line.strip_prefix("content-type: "))
                        .map(str::to_owned)
                }),
                data: part[split + 4..].to_vec(),
            });
            rest = rest_part;
        }
        parts
    }

    pub(super) fn text(&self, name: &str) -> Option<String> {
        self.parts()
            .into_iter()
            .find(|part| part.name == name)
            .map(|part| String::from_utf8(part.data).unwrap())
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn attribute(headers: &str, key: &str) -> Option<String> {
    let marker = format!("; {key}=\"");
    let start = headers.find(&marker)? + marker.len();
    let end = headers[start..].find('"')?;
    Some(headers[start..start + end].to_owned())
}

/// The endpoint's state, shared with the test.
pub(super) struct FakeStt {
    script: Mutex<VecDeque<Reply>>,
    received: Mutex<Vec<Received>>,
    inflight: AtomicUsize,
    peak: AtomicUsize,
    gate: watch::Sender<bool>,
}

impl FakeStt {
    /// Serve the script in order, then `{"text": SPOKEN}` for every request.
    pub(super) fn script(&self, replies: impl IntoIterator<Item = Reply>) {
        self.script.lock().unwrap().extend(replies);
    }

    pub(super) fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    pub(super) fn requests(&self) -> usize {
        self.received.lock().unwrap().len()
    }

    /// Most requests held at once.
    pub(super) fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    /// Hold every request until [`FakeStt::open`].
    pub(super) fn close(&self) {
        self.gate.send_replace(false);
    }

    pub(super) fn open(&self) {
        self.gate.send_replace(true);
    }
}

async fn transcribe(
    State(fake): State<Arc<FakeStt>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let header = |name| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    fake.received.lock().unwrap().push(Received {
        authorization: header(header::AUTHORIZATION),
        content_type: header(header::CONTENT_TYPE).unwrap_or_default(),
        content_length: header(header::CONTENT_LENGTH).and_then(|value| value.parse().ok()),
        body: body.to_vec(),
    });
    let held = fake.inflight.fetch_add(1, Ordering::SeqCst) + 1;
    fake.peak.fetch_max(held, Ordering::SeqCst);
    let mut gate = fake.gate.subscribe();
    let _ = gate.wait_for(|open| *open).await;
    let reply = fake
        .script
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| Reply::ok(&format!("{{\"text\":\"{SPOKEN}\"}}")));
    tokio::time::sleep(reply.delay).await;
    fake.inflight.fetch_sub(1, Ordering::SeqCst);
    (
        StatusCode::from_u16(reply.status).unwrap(),
        [(header::CONTENT_TYPE, "application/json")],
        reply.body,
    )
}

/// Start the endpoint on its own thread and return its transcription URL.
pub(super) fn start() -> (String, Arc<FakeStt>) {
    isolated(|| async {
        let fake = Arc::new(FakeStt {
            script: Mutex::default(),
            received: Mutex::default(),
            inflight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            gate: watch::Sender::new(true),
        });
        let router = Router::new()
            .route(PATH, post(transcribe))
            .layer(DefaultBodyLimit::disable())
            .with_state(fake.clone());
        (format!("{}{PATH}", serve(router).await), fake)
    })
}

/// Settings for `url` with the platform key and `extra` keys.
pub(super) fn config_for(
    url: &str,
    extra: &[(&'static str, &str)],
) -> Result<SttConfig, ConfigError> {
    let mut pairs: Vec<(&'static str, String)> = vec![
        ("CMAEG_STT_URL", url.to_owned()),
        ("CMAEG_STT_API_KEY", KEY.to_owned()),
    ];
    for (key, value) in extra {
        pairs.retain(|(name, _)| name != key);
        pairs.push((key, (*value).to_owned()));
    }
    SttConfig::from_lookup(|key| {
        Ok(pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.clone()))
    })
}

/// A transcriber for `url` under the settings `extra` adds.
pub(super) fn transcriber(
    url: &str,
    extra: &[(&'static str, &str)],
) -> Arc<OpenAiCompatibleTranscriber> {
    let config = config_for(url, extra).unwrap();
    let endpoint = config.endpoint.as_ref().expect("an endpoint");
    Arc::new(OpenAiCompatibleTranscriber::new(endpoint, &config).unwrap())
}

/// An Ogg body of `length` bytes, by its capture pattern.
pub(super) fn ogg(length: usize) -> Vec<u8> {
    let mut bytes = b"OggS\0\x02".to_vec();
    bytes.resize(length, 0);
    bytes
}

pub(super) fn input(container: AudioContainer, bytes: Vec<u8>) -> AudioInput {
    AudioInput {
        bytes: bytes.into(),
        container,
    }
}
