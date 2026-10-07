//! The speech-to-text seam.
//!
//! Callers depend on [`Transcriber`] only. [`DisabledTranscriber`] is the
//! baseline: with no speech-to-text configured, a transcript is refused as
//! `transcriber_disabled` before anything is fetched;
//! [`openai::OpenAiCompatibleTranscriber`] is selected once an endpoint is
//! configured. [`meter`] decides what a transcription costs and whether a
//! channel may still spend, and [`upload`] turns a fetched original into the
//! upload a transcriber receives.
pub mod meter;
pub mod openai;
pub mod upload;

use bytes::Bytes;
use futures_util::future::BoxFuture;
use std::fmt;
use tokio::time::Instant;

/// An audio container a transcriber may accept, from the sniffed type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AudioContainer {
    Mp3,
    Mp4,
    M4a,
    Wav,
    Webm,
    Ogg,
    Flac,
}

impl AudioContainer {
    /// The container of a sniffed MIME type. AMR, QuickTime, Matroska and
    /// everything else no transcriber accepts are `None`.
    pub fn from_sniffed(mime: &str) -> Option<Self> {
        match mime {
            "audio/mpeg" => Some(Self::Mp3),
            "video/mp4" => Some(Self::Mp4),
            "audio/m4a" | "audio/mp4" => Some(Self::M4a),
            "audio/x-wav" | "audio/wav" => Some(Self::Wav),
            "video/webm" | "audio/webm" => Some(Self::Webm),
            "audio/ogg" | "audio/opus" => Some(Self::Ogg),
            "audio/x-flac" | "audio/flac" => Some(Self::Flac),
            _ => None,
        }
    }

    /// The format name speech-to-text APIs use.
    pub fn format(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Mp4 => "mp4",
            Self::M4a => "m4a",
            Self::Wav => "wav",
            Self::Webm => "webm",
            Self::Ogg => "ogg",
            Self::Flac => "flac",
        }
    }

    /// The extension of the upload file name, `audio.<ext>`.
    pub fn extension(&self) -> &'static str {
        self.format()
    }

    /// The MIME type the upload is declared as.
    pub fn upload_mime(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Mp4 => "video/mp4",
            Self::M4a => "audio/mp4",
            Self::Wav => "audio/wav",
            Self::Webm => "audio/webm",
            Self::Ogg => "audio/ogg",
            Self::Flac => "audio/flac",
        }
    }
}

/// Audio bytes in a sniffed container.
pub struct AudioInput {
    pub bytes: Bytes,
    pub container: AudioContainer,
}

/// Content must never reach a log line, so only the size is shown.
impl fmt::Debug for AudioInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AudioInput")
            .field("bytes", &self.bytes.len())
            .field("container", &self.container)
            .finish()
    }
}

/// A transcription, before normalization and paging.
pub struct Transcript {
    pub text: String,
    /// The audio duration the provider billed, when it reports one.
    pub usage_seconds: Option<u32>,
}

/// Content must never reach a log line, so only the size is shown.
impl fmt::Debug for Transcript {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Transcript")
            .field("bytes", &self.text.len())
            .field("usage_seconds", &self.usage_seconds)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscribeError {
    /// No transcriber is configured.
    Disabled,
    /// The transcriber does not accept this container.
    Unsupported,
    TimedOut,
    RateLimited,
    Failed,
}

/// Speech to text.
pub trait Transcriber: Send + Sync {
    /// Whether transcripts can be produced at all; callers refuse without
    /// fetching when they cannot.
    fn enabled(&self) -> bool;
    fn accepts(&self, container: AudioContainer) -> bool;
    fn transcribe<'a>(
        &'a self,
        input: AudioInput,
        deadline: Instant,
    ) -> BoxFuture<'a, Result<Transcript, TranscribeError>>;
}

/// The transcriber when none is configured.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisabledTranscriber;

impl Transcriber for DisabledTranscriber {
    fn enabled(&self) -> bool {
        false
    }

    fn accepts(&self, _container: AudioContainer) -> bool {
        false
    }

    fn transcribe<'a>(
        &'a self,
        _input: AudioInput,
        _deadline: Instant,
    ) -> BoxFuture<'a, Result<Transcript, TranscribeError>> {
        Box::pin(async { Err(TranscribeError::Disabled) })
    }
}
