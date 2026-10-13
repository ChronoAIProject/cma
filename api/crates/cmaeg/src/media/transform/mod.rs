//! Sandboxed FFmpeg jobs: keyframes from video and 16 kHz mono PCM from any
//! audio container.
//!
//! FFmpeg is a large C decoder that parses hostile input, so it never runs in
//! the gateway process. Each job is a fresh exec of the gateway binary that
//! limits itself, installs two seccomp filters and then executes FFmpeg or
//! ffprobe once with a fixed argv. The sandbox exists on Linux x86_64 only;
//! elsewhere both job modes exit 125 without executing anything.
//!
//! Callers depend on [`MediaTransformer`] only. [`NoTransformer`] refuses
//! everything; [`ffmpeg::FfmpegJobTransformer`] runs the jobs over an
//! injected [`launcher::JobLauncher`]. [`setup`] proves the sandbox at
//! startup, and [`adapters`] put the transformer behind the media seams.
//! [`self_test`] runs the committed fixtures through the real path when the
//! image is built.
pub mod adapters;
pub mod argv;
pub mod bin_dir;
pub mod ffmpeg;
pub mod jobdir;
pub mod launcher;
pub mod outcome;
pub mod pcm;
pub mod probe_json;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod sandbox;
pub mod scheduler;
pub mod self_test;
pub mod setup;
pub mod watchdog;

pub use jobdir::MEDIA_ROOT;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use sandbox::{job_main, sandbox_check_main};

use bytes::Bytes;
use cmaeg_core::ids::ChannelId;
use futures_util::future::BoxFuture;
use outcome::{StderrClass, TransformOutcome};
use std::fmt;

/// Keyframes of one video, fetched by the caller.
#[derive(Clone)]
pub struct KeyframeRequest {
    pub channel: ChannelId,
    pub input: Bytes,
    /// How many frames to spread over the duration, `1..=6`.
    pub frames: u8,
}

/// The first `seconds` of an audio track as PCM.
#[derive(Clone)]
pub struct AudioRequest {
    pub channel: ChannelId,
    pub input: Bytes,
    pub seconds: u16,
}

/// Media must never reach a log line, so only sizes are shown.
impl fmt::Debug for KeyframeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeyframeRequest")
            .field("input_bytes", &self.input.len())
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

/// Media must never reach a log line, so only sizes are shown.
impl fmt::Debug for AudioRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AudioRequest")
            .field("input_bytes", &self.input.len())
            .field("seconds", &self.seconds)
            .finish_non_exhaustive()
    }
}

/// The JPEG keyframes of one set, ordered by position.
pub struct Keyframes {
    /// Each JPEG and its position in milliseconds.
    pub frames: Vec<(Bytes, u64)>,
    /// The container duration, when the probe reported a usable one.
    pub duration_ms: Option<u64>,
    /// Some planned position was skipped or not reached.
    pub partial: bool,
}

/// Media must never reach a log line, so only sizes are shown.
impl fmt::Debug for Keyframes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let positions: Vec<u64> = self.frames.iter().map(|(_, at_ms)| *at_ms).collect();
        formatter
            .debug_struct("Keyframes")
            .field("positions_ms", &positions)
            .field("duration_ms", &self.duration_ms)
            .field("partial", &self.partial)
            .finish()
    }
}

/// 16 kHz mono 16-bit PCM in a WAV container.
pub struct Pcm {
    pub wav: Bytes,
    /// The PCM bytes after the 44-byte header.
    pub data_len: usize,
    /// The output reached its cap, so the audio may be longer.
    pub truncated: bool,
}

/// Media must never reach a log line, so only sizes are shown.
impl fmt::Debug for Pcm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pcm")
            .field("data_len", &self.data_len)
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// Why a transform produced nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformError {
    /// A job ended this way; its stderr class refines the reason.
    Job {
        outcome: TransformOutcome,
        stderr_class: StderrClass,
    },
    /// The set's deadline ended a keyframe set before any frame.
    NoFrames,
    /// No transformer is configured.
    Unavailable,
}

impl TransformError {
    /// A failure with nothing learned from stderr.
    pub fn job(outcome: TransformOutcome) -> Self {
        Self::Job {
            outcome,
            stderr_class: StderrClass::None,
        }
    }
}

/// Turns fetched media into what an agent or a transcriber can consume.
pub trait MediaTransformer: Send + Sync + 'static {
    fn keyframes(
        &self,
        request: KeyframeRequest,
    ) -> BoxFuture<'_, Result<Keyframes, TransformError>>;
    fn audio(&self, request: AudioRequest) -> BoxFuture<'_, Result<Pcm, TransformError>>;
}

/// The transformer while none is configured: it refuses everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTransformer;

impl MediaTransformer for NoTransformer {
    fn keyframes(
        &self,
        _request: KeyframeRequest,
    ) -> BoxFuture<'_, Result<Keyframes, TransformError>> {
        Box::pin(async { Err(TransformError::Unavailable) })
    }

    fn audio(&self, _request: AudioRequest) -> BoxFuture<'_, Result<Pcm, TransformError>> {
        Box::pin(async { Err(TransformError::Unavailable) })
    }
}

/// The exit status of a job whose sandbox is unavailable.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
const SANDBOX_UNAVAILABLE: i32 = 125;

/// `cmaeg --media-job` where no sandbox exists: exit 125 without executing.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn job_main(_args: Vec<std::ffi::OsString>) -> ! {
    eprintln!("cmaeg media-job: the sandbox requires linux x86_64");
    std::process::exit(SANDBOX_UNAVAILABLE)
}

/// `cmaeg --media-sandbox-check` where no sandbox exists: always fails.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn sandbox_check_main() -> ! {
    eprintln!("cmaeg media-sandbox-check: the sandbox requires linux x86_64");
    std::process::exit(SANDBOX_UNAVAILABLE)
}
