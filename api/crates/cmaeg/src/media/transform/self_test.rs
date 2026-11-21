//! `cmaeg --media-self-test <fixtures>`: every committed media fixture
//! through the real transformer path (the scheduler, a job directory and the
//! sandboxed launcher), so an image whose FFmpeg cannot serve the fixed argv
//! under the sandbox fails its own build instead of reaching a pod.
//!
//! Cases run one after another. A report line carries an outcome, counts and
//! a duration only: never media content and never a job's stderr, which can
//! quote its input.
use super::{
    ffmpeg::FfmpegJobTransformer,
    launcher::{JobLauncher, JobResult, JobSpec},
    outcome::TransformOutcome,
    pcm::{write_wav_header, WAV_HEADER_BYTES},
    AudioRequest, KeyframeRequest, Keyframes, MediaTransformer, Pcm,
};
use crate::media::image::{self, PREVIEW};
use bytes::{Bytes, BytesMut};
use cmaeg_core::ids::ChannelId;
use futures_util::future::BoxFuture;
use std::{
    io::Cursor,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::{sync::Semaphore, time::Instant};

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod cli;
mod report;
#[cfg(test)]
mod tests;

pub use report::{CaseLine, CaseOp, CaseOutcome, Report};

/// The gateway's flag for the self-test.
pub const SELF_TEST_FLAG: &str = "--media-self-test";
/// Every case passed.
pub const EXIT_PASSED: i32 = 0;
/// Some case failed.
pub const EXIT_FAILED: i32 = 1;
/// The arguments or the configuration are unusable.
pub const EXIT_USAGE: i32 = 2;
/// No sandbox exists on this platform.
pub const EXIT_UNSUPPORTED: i32 = 125;
/// The scheduler keys sets by channel; the self-test is one channel.
const CHANNEL: &str = "cmaeg1.ch.media-self-test";
/// How long one returned frame may take to normalize.
const NORMALIZE_WALL: Duration = Duration::from_secs(10);
/// The job outcomes no case may see: each means a limit or the sandbox, not
/// the fixture, ended a job.
const FORBIDDEN: [TransformOutcome; 4] = [
    TransformOutcome::CpuLimit,
    TransformOutcome::ResourceLimit,
    TransformOutcome::Killed,
    TransformOutcome::SandboxUnavailable,
];

/// One committed fixture and what is asked of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Case {
    /// The file name under the fixtures directory.
    pub file: &'static str,
    /// Keyframes requested, when the fixture has video.
    pub keyframes: Option<u8>,
    /// Seconds of PCM requested, when the fixture has audio.
    pub audio_seconds: Option<u16>,
    /// The least PCM accepted; any PCM at all when zero.
    pub min_pcm_bytes: usize,
    /// Every job must end `ok` and every requested frame must be returned:
    /// the case proves a decode fits the job's limits, so a skipped frame is
    /// a failure rather than sparse keyframes.
    pub exact: bool,
}

/// One case per fixture in `tests/fixtures/media`.
pub const CASES: &[Case] = &[
    Case {
        file: "h264_aac.mp4",
        keyframes: Some(6),
        audio_seconds: Some(5),
        min_pcm_bytes: 0,
        exact: false,
    },
    Case {
        file: "vp9_opus.webm",
        keyframes: Some(6),
        audio_seconds: Some(5),
        min_pcm_bytes: 0,
        exact: false,
    },
    Case {
        file: "voice.ogg",
        keyframes: None,
        audio_seconds: Some(5),
        // Two of its three seconds, as 16 kHz mono PCM.
        min_pcm_bytes: 2 * 32_000,
        exact: false,
    },
    Case {
        file: "tone.mp3",
        keyframes: None,
        audio_seconds: Some(5),
        min_pcm_bytes: 0,
        exact: false,
    },
    Case {
        file: "tone.amr",
        keyframes: None,
        audio_seconds: Some(5),
        min_pcm_bytes: 0,
        exact: false,
    },
    // With `-skip_frame nokey` a GIF may yield fewer frames than asked.
    Case {
        file: "anim.gif",
        keyframes: Some(6),
        audio_seconds: None,
        min_pcm_bytes: 0,
        exact: false,
    },
    // 3840×2160 is exactly `-max_pixels`: the frame proves 4K HEVC decodes
    // within RLIMIT_AS.
    Case {
        file: "hevc_4k.mp4",
        keyframes: Some(1),
        audio_seconds: None,
        min_pcm_bytes: 0,
        exact: true,
    },
];

/// One job the recording launcher ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobRecord {
    /// The job's op label: `probe`, `frame` or `audio`.
    pub op: &'static str,
    /// The launcher's outcome, before the transformer validates the output.
    pub outcome: TransformOutcome,
}

/// A launcher that records the outcome of every job it runs, so the
/// self-test sees the per-job outcomes a partial keyframe set does not
/// return.
pub struct RecordingLauncher<L: JobLauncher> {
    inner: L,
    jobs: Mutex<Vec<JobRecord>>,
}

impl<L: JobLauncher> RecordingLauncher<L> {
    pub fn new(inner: L) -> Self {
        Self {
            inner,
            jobs: Mutex::new(Vec::new()),
        }
    }

    /// Every job recorded since the last call, in the order they ran.
    pub fn take(&self) -> Vec<JobRecord> {
        std::mem::take(&mut *self.lock())
    }

    fn lock(&self) -> MutexGuard<'_, Vec<JobRecord>> {
        // A push cannot leave the list inconsistent, so a poisoned lock
        // still guards a valid list.
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl<L: JobLauncher> JobLauncher for RecordingLauncher<L> {
    fn run(&self, spec: JobSpec) -> BoxFuture<'_, JobResult> {
        Box::pin(async move {
            let op = spec.op.as_str();
            let result = self.inner.run(spec).await;
            self.lock().push(JobRecord {
                op,
                outcome: result.outcome,
            });
            result
        })
    }
}

/// Run every case of [`CASES`] from `fixtures`, one after another, through
/// `transformer`. `decode` is the pod's decode permit, which every returned
/// frame is normalized under.
pub async fn run<L: JobLauncher>(
    transformer: &FfmpegJobTransformer<RecordingLauncher<L>>,
    fixtures: &Path,
    decode: &Arc<Semaphore>,
) -> Report {
    let mut lines = Vec::new();
    for case in CASES {
        if let Some(frames) = case.keyframes {
            lines.push(keyframes_case(transformer, fixtures, decode, case, frames).await);
        }
        if let Some(seconds) = case.audio_seconds {
            lines.push(audio_case(transformer, fixtures, case, seconds).await);
        }
    }
    Report { lines }
}

async fn keyframes_case<L: JobLauncher>(
    transformer: &FfmpegJobTransformer<RecordingLauncher<L>>,
    fixtures: &Path,
    decode: &Arc<Semaphore>,
    case: &Case,
    frames: u8,
) -> CaseLine {
    let started = Instant::now();
    let mut line = line(case, CaseOp::Keyframes);
    let Some(input) = read(fixtures, case) else {
        return finish(line, CaseOutcome::FixtureUnreadable, started);
    };
    drop(transformer.launcher().take());
    let result = transformer
        .keyframes(KeyframeRequest {
            channel: channel(),
            input,
            frames,
        })
        .await;
    let jobs = transformer.launcher().take();
    line.frames = result.as_ref().map_or(0, |set| set.frames.len());
    let outcome = match job_failure(&jobs, case, |job| job.op == "probe") {
        Some(outcome) => outcome,
        None => match result {
            Err(error) => CaseOutcome::of(error),
            Ok(set) => judge_frames(set, frames, case.exact, decode).await,
        },
    };
    finish(line, outcome, started)
}

async fn audio_case<L: JobLauncher>(
    transformer: &FfmpegJobTransformer<RecordingLauncher<L>>,
    fixtures: &Path,
    case: &Case,
    seconds: u16,
) -> CaseLine {
    let started = Instant::now();
    let mut line = line(case, CaseOp::Audio);
    let Some(input) = read(fixtures, case) else {
        return finish(line, CaseOutcome::FixtureUnreadable, started);
    };
    drop(transformer.launcher().take());
    let result = transformer
        .audio(AudioRequest {
            channel: channel(),
            input,
            seconds,
        })
        .await;
    let jobs = transformer.launcher().take();
    line.pcm_bytes = result.as_ref().map_or(0, |pcm| pcm.data_len);
    // The one audio job must end `ok`, whatever the case's strictness.
    let outcome = match job_failure(&jobs, case, |_| true) {
        Some(outcome) => outcome,
        None => match result {
            Err(error) => CaseOutcome::of(error),
            Ok(pcm) => judge_pcm(&pcm, case.min_pcm_bytes),
        },
    };
    finish(line, outcome, started)
}

/// The first job that fails the case: one that hit a limit or the sandbox,
/// any non-`ok` job of an exact case, or a non-`ok` job that `must_succeed`
/// selects. Other non-`ok` frame jobs were skipped by the set, which goes on.
fn job_failure(
    jobs: &[JobRecord],
    case: &Case,
    must_succeed: impl Fn(&JobRecord) -> bool,
) -> Option<CaseOutcome> {
    jobs.iter()
        .find(|job| {
            FORBIDDEN.contains(&job.outcome)
                || (job.outcome != TransformOutcome::Ok && (case.exact || must_succeed(job)))
        })
        .map(|job| CaseOutcome::Job(job.outcome))
}

/// A returned set passes when the probe found a duration, it holds between
/// one and `requested` frames (exactly `requested` when exact), and every
/// frame is a preview-sized JPEG that normalizes.
async fn judge_frames(
    set: Keyframes,
    requested: u8,
    exact: bool,
    decode: &Arc<Semaphore>,
) -> CaseOutcome {
    if set.duration_ms.is_none() {
        return CaseOutcome::ProbeInvalid;
    }
    let count = set.frames.len();
    if count == 0 || count > usize::from(requested) || (exact && count != usize::from(requested)) {
        return CaseOutcome::FrameCount;
    }
    for (jpeg, _) in set.frames {
        let fits =
            ::image::ImageReader::with_format(Cursor::new(&jpeg), ::image::ImageFormat::Jpeg)
                .into_dimensions()
                .is_ok_and(|(width, height)| width.max(height) <= PREVIEW.max_side);
        if !fits {
            return CaseOutcome::FrameInvalid;
        }
        let deadline = Instant::now() + NORMALIZE_WALL;
        if image::normalize(jpeg, PREVIEW, decode, deadline)
            .await
            .is_err()
        {
            return CaseOutcome::FrameInvalid;
        }
    }
    CaseOutcome::Ok
}

/// PCM passes when it is even, at least `min_bytes` long (any length when
/// zero) and behind exactly the header `pcm.rs` writes for its length.
fn judge_pcm(pcm: &Pcm, min_bytes: usize) -> CaseOutcome {
    let Ok(data_len) = u32::try_from(pcm.data_len) else {
        return CaseOutcome::PcmInvalid;
    };
    let mut header = BytesMut::zeroed(WAV_HEADER_BYTES);
    write_wav_header(&mut header, data_len);
    let valid = pcm.data_len.is_multiple_of(2)
        && pcm.data_len >= min_bytes.max(1)
        && pcm.wav.len() == WAV_HEADER_BYTES + pcm.data_len
        && pcm.wav.get(..WAV_HEADER_BYTES) == Some(&header[..]);
    if valid {
        CaseOutcome::Ok
    } else {
        CaseOutcome::PcmInvalid
    }
}

fn line(case: &Case, op: CaseOp) -> CaseLine {
    CaseLine {
        file: case.file,
        op,
        outcome: CaseOutcome::Ok,
        frames: 0,
        pcm_bytes: 0,
        ms: 0,
    }
}

fn finish(mut line: CaseLine, outcome: CaseOutcome, started: Instant) -> CaseLine {
    line.outcome = outcome;
    line.ms = started.elapsed().as_millis();
    if outcome != CaseOutcome::Ok {
        tracing::warn!(
            case = line.file,
            op = line.op.as_str(),
            outcome = outcome.as_str(),
            "media self-test case failed"
        );
    }
    line
}

fn read(fixtures: &Path, case: &Case) -> Option<Bytes> {
    match std::fs::read(fixtures.join(case.file)) {
        Ok(bytes) => Some(Bytes::from(bytes)),
        Err(error) => {
            tracing::warn!(case = case.file, kind = ?error.kind(), "media self-test fixture unreadable");
            None
        }
    }
}

fn channel() -> ChannelId {
    ChannelId::new(CHANNEL).expect("the self-test channel id has the channel prefix")
}

/// `cmaeg --media-self-test <fixtures>`: run every case and exit
/// [`EXIT_PASSED`], [`EXIT_FAILED`] or [`EXIT_USAGE`].
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn main(fixtures: &Path) -> ! {
    std::process::exit(cli::status(fixtures))
}

/// `cmaeg --media-self-test` where no sandbox exists: exit 125 without
/// executing anything.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn main(_fixtures: &Path) -> ! {
    eprintln!("cmaeg media-self-test: the sandbox requires linux x86_64");
    std::process::exit(EXIT_UNSUPPORTED)
}
