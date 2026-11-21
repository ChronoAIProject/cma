//! Speech to text through an OpenAI-compatible transcription endpoint.
//!
//! One multipart `POST` per attempt carries the audio, the model and
//! `response_format=json`, authorized by the operator's platform key; a
//! tenant's key is never used. The audio part streams the fetched bytes
//! without copying them. The URL, the key, the request and response bodies
//! and the transcript never reach a log line: the caller's `cmaeg.media.stt`
//! span names the call by its channel hash and metered seconds only.
use super::{AudioContainer, AudioInput, TranscribeError, Transcriber, Transcript};
use crate::{
    media::{
        body::{read_capped, Overflow},
        FetchError,
    },
    ConfigError, SttConfig, SttEndpoint,
};
use cmaeg_core::channel::Secret;
use futures_util::future::BoxFuture;
use reqwest::{
    multipart::{Form, Part},
    StatusCode, Url,
};
use serde::Deserialize;
use std::{collections::BTreeSet, fmt, sync::Arc, time::Duration};
use tokio::{
    sync::Semaphore,
    time::{sleep_until, timeout_at, Instant},
};

/// A pod-wide bound on calls in flight. Every media job may reach the
/// transcriber at once, and each call holds its upload until the endpoint
/// answers, so four keeps the platform key's concurrency and the bytes held
/// for uploads small; further calls wait for a permit within their deadline.
pub const MAX_CONCURRENT_CALLS: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Far above any transcript of the longest metered call; a larger body is
/// not a transcription response.
const RESPONSE_CAP: u64 = 256 * 1024;
const RETRY_PAUSE: Duration = Duration::from_millis(500);
/// A retry is attempted only when at least this much of the deadline is left.
const RETRY_MIN_REMAINING: Duration = Duration::from_secs(2);
/// A day of audio: any reported duration beyond it is not credible.
const MAX_USAGE_SECONDS: f64 = 86_400.0;

/// The OpenAI speech-to-text API, or any endpoint speaking its protocol.
pub struct OpenAiCompatibleTranscriber {
    http: reqwest::Client,
    url: Url,
    model: String,
    key: Secret,
    formats: BTreeSet<AudioContainer>,
    timeout: Duration,
    concurrency: Arc<Semaphore>,
}

/// Only the model, formats and timeout: the URL and key never reach a log.
impl fmt::Debug for OpenAiCompatibleTranscriber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatibleTranscriber")
            .field("model", &self.model)
            .field("formats", &self.formats)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatibleTranscriber {
    /// A transcriber for `endpoint`, with the model, formats and timeout of
    /// `config`.
    pub fn new(endpoint: &SttEndpoint, config: &SttConfig) -> Result<Self, ConfigError> {
        let http = reqwest::Client::builder()
            .use_rustls_tls()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|_| ConfigError::InvalidMedia {
                detail: "cannot configure the speech-to-text client".into(),
            })?;
        Ok(Self {
            http,
            url: endpoint.url.clone(),
            model: config.model.clone(),
            key: endpoint.api_key.clone(),
            formats: config
                .formats
                .iter()
                .filter_map(|name| container(name))
                .collect(),
            timeout: config.timeout,
            concurrency: Arc::new(Semaphore::new(MAX_CONCURRENT_CALLS)),
        })
    }

    /// One request and its response, before the per-attempt `deadline`.
    async fn attempt(&self, input: &AudioInput, deadline: Instant) -> Attempt {
        let length = input.bytes.len() as u64;
        // A clone of `Bytes` shares the fetched buffer: the part streams the
        // original without copying it.
        let file = match Part::stream_with_length(reqwest::Body::from(input.bytes.clone()), length)
            .file_name(format!("audio.{}", input.container.extension()))
            .mime_str(input.container.upload_mime())
        {
            Ok(file) => file,
            Err(_) => return Attempt::Done(Err(TranscribeError::Failed)),
        };
        let form = Form::new()
            .text("model", self.model.clone())
            .text("response_format", "json")
            .part("file", file);
        let request = self
            .http
            .post(self.url.clone())
            .bearer_auth(self.key.as_str())
            .multipart(form);
        let response = match timeout_at(deadline, request.send()).await {
            Err(_) => return Attempt::Done(Err(TranscribeError::TimedOut)),
            Ok(Err(error)) => return Attempt::Done(Err(transport(error))),
            Ok(Ok(response)) => response,
        };
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Attempt::Retryable(TranscribeError::RateLimited, status);
        }
        if status.is_server_error() {
            return Attempt::Retryable(TranscribeError::Failed, status);
        }
        if status != StatusCode::OK {
            tracing::info!(
                status = status.as_u16(),
                "speech-to-text endpoint refused the request"
            );
            return Attempt::Done(Err(TranscribeError::Failed));
        }
        let body = match read_capped(response, RESPONSE_CAP, Overflow::Reject, deadline).await {
            Ok(body) => body,
            Err(FetchError::TimedOut) => return Attempt::Done(Err(TranscribeError::TimedOut)),
            Err(error) => {
                tracing::info!(
                    reason = error.as_str(),
                    "speech-to-text response unreadable"
                );
                return Attempt::Done(Err(TranscribeError::Failed));
            }
        };
        Attempt::Done(parse(&body.bytes))
    }
}

impl Transcriber for OpenAiCompatibleTranscriber {
    fn enabled(&self) -> bool {
        true
    }

    fn accepts(&self, container: AudioContainer) -> bool {
        self.formats.contains(&container)
    }

    /// Retry once on 429 or 5xx, after a short pause, when enough of the
    /// deadline is left for a second attempt to finish; any other refusal is
    /// final at once.
    fn transcribe<'a>(
        &'a self,
        input: AudioInput,
        deadline: Instant,
    ) -> BoxFuture<'a, Result<Transcript, TranscribeError>> {
        Box::pin(async move {
            if !self.accepts(input.container) {
                return Err(TranscribeError::Unsupported);
            }
            let _permit = match timeout_at(deadline, self.concurrency.acquire()).await {
                Err(_) => return Err(TranscribeError::TimedOut),
                Ok(Err(_)) => return Err(TranscribeError::Failed),
                Ok(Ok(permit)) => permit,
            };
            let mut retried = false;
            loop {
                let attempt_deadline = deadline.min(Instant::now() + self.timeout);
                let (error, status) = match self.attempt(&input, attempt_deadline).await {
                    Attempt::Done(result) => return result,
                    Attempt::Retryable(error, status) => (error, status),
                };
                let remaining = deadline.saturating_duration_since(Instant::now());
                if retried || remaining < RETRY_MIN_REMAINING {
                    tracing::info!(
                        status = status.as_u16(),
                        retried,
                        "speech-to-text endpoint unavailable"
                    );
                    return Err(error);
                }
                tracing::debug!(
                    status = status.as_u16(),
                    "speech-to-text endpoint unavailable; retrying once"
                );
                retried = true;
                sleep_until((Instant::now() + RETRY_PAUSE).min(deadline)).await;
            }
        })
    }
}

/// The outcome of one attempt: final, or worth one retry.
enum Attempt {
    Done(Result<Transcript, TranscribeError>),
    Retryable(TranscribeError, StatusCode),
}

/// The JSON response. Unknown fields are ignored, and `usage` is read
/// leniently: the API does not document a duration, and models that report
/// tokens instead must not fail an otherwise good transcript.
#[derive(Deserialize)]
struct SttResponse {
    text: String,
    #[serde(default)]
    usage: Option<serde_json::Value>,
}

fn parse(body: &[u8]) -> Result<Transcript, TranscribeError> {
    let response: SttResponse = serde_json::from_slice(body).map_err(|_| {
        tracing::info!("speech-to-text response is not a transcription");
        TranscribeError::Failed
    })?;
    Ok(Transcript {
        usage_seconds: usage_seconds(response.usage.as_ref()),
        text: response.text,
    })
}

/// The billed duration, when `usage` is `{"type": "duration", "seconds": s}`
/// with a credible `s`, rounded up to whole seconds.
fn usage_seconds(usage: Option<&serde_json::Value>) -> Option<u32> {
    let usage = usage?;
    if usage.get("type")?.as_str()? != "duration" {
        return None;
    }
    let seconds = usage.get("seconds")?.as_f64()?;
    (0.0..=MAX_USAGE_SECONDS)
        .contains(&seconds)
        .then(|| seconds.ceil() as u32)
}

/// The container a configured format name admits. `mpeg` and `mpga` are the
/// API's names for MP3.
fn container(format: &str) -> Option<AudioContainer> {
    match format {
        "mp3" | "mpeg" | "mpga" => Some(AudioContainer::Mp3),
        "mp4" => Some(AudioContainer::Mp4),
        "m4a" => Some(AudioContainer::M4a),
        "wav" => Some(AudioContainer::Wav),
        "webm" => Some(AudioContainer::Webm),
        "ogg" => Some(AudioContainer::Ogg),
        "flac" => Some(AudioContainer::Flac),
        _ => None,
    }
}

/// Classify a transport failure; the error loses its URL before anything can
/// render it.
fn transport(error: reqwest::Error) -> TranscribeError {
    let error = error.without_url();
    if error.is_timeout() {
        return TranscribeError::TimedOut;
    }
    tracing::info!(%error, "speech-to-text request failed in transport");
    TranscribeError::Failed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_credible_duration_usage_is_read() {
        let read = |usage: serde_json::Value| usage_seconds(Some(&usage));
        assert_eq!(read(json!({"type": "duration", "seconds": 12.2})), Some(13));
        assert_eq!(read(json!({"type": "duration", "seconds": 0})), Some(0));
        assert_eq!(
            read(json!({"type": "duration", "seconds": 86400})),
            Some(86_400)
        );
        assert_eq!(read(json!({"type": "duration", "seconds": 86400.5})), None);
        assert_eq!(read(json!({"type": "duration", "seconds": -1})), None);
        assert_eq!(read(json!({"type": "duration", "seconds": "12"})), None);
        assert_eq!(read(json!({"type": "duration"})), None);
        assert_eq!(read(json!({"type": "tokens", "input_tokens": 5})), None);
        assert_eq!(read(json!(42)), None);
        assert_eq!(usage_seconds(None), None);
    }

    #[test]
    fn responses_need_text_and_tolerate_any_usage() {
        let parsed =
            parse(br#"{"text":"hello","usage":{"type":"tokens","total_tokens":3},"logprobs":[]}"#)
                .unwrap();
        assert_eq!(
            (parsed.text.as_str(), parsed.usage_seconds),
            ("hello", None)
        );
        let parsed = parse(br#"{"text":"","usage":{"type":"duration","seconds":1.5}}"#).unwrap();
        assert_eq!(parsed.usage_seconds, Some(2));
        for body in [&b"{}"[..], br#"{"text":7}"#, b"not json", b""] {
            assert_eq!(parse(body).unwrap_err(), TranscribeError::Failed);
        }
    }

    #[test]
    fn mp3_aliases_admit_the_mp3_container() {
        for name in ["mp3", "mpeg", "mpga"] {
            assert_eq!(container(name), Some(AudioContainer::Mp3));
        }
        for name in crate::config::KNOWN_FORMATS {
            assert!(container(name).is_some(), "{name}");
        }
        assert_eq!(container("amr"), None);
    }
}
