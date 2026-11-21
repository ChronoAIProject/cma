//! [`FfmpegJobTransformer`]: keyframe and audio sets over a [`JobLauncher`].
//!
//! Each set takes the pod's one transform slot, writes its input into a fresh
//! job directory and runs its jobs one after another under the set's
//! deadline. Every job's stdout cap is reserved in the pod's in-flight byte
//! budget before it is spawned and released once its output is converted.
//! Nothing a job writes is logged: only outcomes, durations and sizes.
use super::{
    argv::{AudioSeconds, FrameTime, JobOp},
    jobdir::{JobDir, MediaRoot},
    launcher::{observe, JobLauncher, JobResult, JobSpec},
    outcome::TransformOutcome,
    pcm::{pcm_bytes, write_wav_header, WAV_HEADER_BYTES},
    probe_json::ProbeOutput,
    scheduler::{SetClass, TransformScheduler},
    AudioRequest, KeyframeRequest, Keyframes, MediaTransformer, Pcm, TransformError,
};
use crate::media::budget::{InflightBytes, InflightPermit};
use bytes::{Bytes, BytesMut};
use futures_util::future::BoxFuture;
use std::io;
use tokio::time::Instant;

/// ffprobe's JSON for one file is a few hundred bytes.
pub const PROBE_STDOUT_CAP: usize = 16_384;
/// A 768 px JPEG at quality 5 stays well below this.
pub const FRAME_STDOUT_CAP: usize = 262_144;
/// The most keyframes one set extracts.
pub const MAX_FRAMES: u8 = 6;

/// Keyframes and PCM from sandboxed FFmpeg jobs.
pub struct FfmpegJobTransformer<L: JobLauncher> {
    launcher: L,
    root: MediaRoot,
    scheduler: TransformScheduler,
    inflight: InflightBytes,
    video_frames: u8,
}

impl<L: JobLauncher> FfmpegJobTransformer<L> {
    /// `video_frames` caps every request's frame count; `inflight` is the
    /// pod's in-flight byte budget, shared with every fetch.
    pub fn new(
        launcher: L,
        root: MediaRoot,
        scheduler: TransformScheduler,
        inflight: InflightBytes,
        video_frames: u8,
    ) -> Self {
        Self {
            launcher,
            root,
            scheduler,
            inflight,
            video_frames: video_frames.clamp(1, MAX_FRAMES),
        }
    }

    /// The injected launcher, so the build-time self-test can read what its
    /// recording launcher saw.
    pub fn launcher(&self) -> &L {
        &self.launcher
    }

    async fn keyframe_set(&self, request: KeyframeRequest) -> Result<Keyframes, TransformError> {
        let ticket = self
            .scheduler
            .acquire(&request.channel, SetClass::Keyframes)
            .await
            .map_err(|_| TransformError::NoFrames)?;
        let deadline = ticket.deadline();
        let dir = self.open_dir(request.input).await?;
        let wanted = request.frames.clamp(1, self.video_frames);
        let result = self.frames(&dir, wanted, deadline).await;
        // The slot is released before the directory is removed; the next
        // set's sweep removes it too if this removal has not finished.
        drop(ticket);
        close_dir(dir).await;
        result
    }

    async fn frames(
        &self,
        dir: &JobDir,
        wanted: u8,
        deadline: Instant,
    ) -> Result<Keyframes, TransformError> {
        let (probe, held) = self
            .run(
                dir,
                JobOp::Probe,
                deadline,
                (PROBE_STDOUT_CAP, 0),
                |stdout| ProbeOutput::parse(stdout).is_some(),
            )
            .await;
        let duration = match probe.outcome {
            TransformOutcome::Ok => {
                ProbeOutput::parse(&probe.stdout).and_then(|output| output.duration_seconds())
            }
            // A probe that could not describe the input still lets one
            // frame at the start be tried.
            TransformOutcome::ExitNonzero | TransformOutcome::OutputInvalid => None,
            outcome => {
                return Err(TransformError::Job {
                    outcome,
                    stderr_class: probe.stderr_class,
                })
            }
        };
        drop((probe, held));
        let positions = positions(duration, wanted);
        let mut frames = Vec::with_capacity(positions.len());
        let mut last_error = None;
        let mut stopped = false;
        for at_ms in positions.iter().copied() {
            if Instant::now() >= deadline {
                stopped = true;
                break;
            }
            // Positions lie below the probed duration, at most one day.
            let Ok(time) = FrameTime::from_millis(at_ms) else {
                continue;
            };
            let (result, held) = self
                .run(
                    dir,
                    JobOp::Frame(time),
                    deadline,
                    (FRAME_STDOUT_CAP, 0),
                    is_jpeg,
                )
                .await;
            if result.outcome == TransformOutcome::Ok {
                // A right-sized copy, so the frame does not pin the job's
                // whole stdout buffer once its reservation is released.
                frames.push((Bytes::copy_from_slice(&result.stdout), at_ms));
                drop((result, held));
                continue;
            }
            // A sparse-keyframe source can yield an empty frame at one
            // position; the set goes on with the next one.
            last_error = Some(TransformError::Job {
                outcome: result.outcome,
                stderr_class: result.stderr_class,
            });
            if Instant::now() >= deadline {
                stopped = true;
                break;
            }
        }
        if frames.is_empty() {
            return Err(match last_error {
                Some(error) if !stopped => error,
                _ => TransformError::NoFrames,
            });
        }
        Ok(Keyframes {
            partial: frames.len() < positions.len(),
            frames,
            duration_ms: duration.map(|seconds| (seconds * 1_000.0).round() as u64),
        })
    }

    async fn audio_set(&self, request: AudioRequest) -> Result<Pcm, TransformError> {
        let Ok(seconds) = AudioSeconds::new(request.seconds) else {
            tracing::warn!(
                seconds = request.seconds,
                "media audio transcode refused: length out of range"
            );
            return Err(TransformError::Unavailable);
        };
        let ticket = self
            .scheduler
            .acquire(&request.channel, SetClass::Audio)
            .await
            .map_err(|_| TransformError::job(TransformOutcome::Timeout))?;
        let deadline = ticket.deadline();
        let dir = self.open_dir(request.input).await?;
        let cap = pcm_bytes(seconds.get());
        let (result, held) = self
            .run(
                &dir,
                JobOp::Audio(seconds),
                deadline,
                (cap, WAV_HEADER_BYTES),
                |pcm| !pcm.is_empty() && pcm.len() % 2 == 0,
            )
            .await;
        drop(ticket);
        close_dir(dir).await;
        if result.outcome != TransformOutcome::Ok {
            return Err(TransformError::Job {
                outcome: result.outcome,
                stderr_class: result.stderr_class,
            });
        }
        let data_len = result.stdout.len() - WAV_HEADER_BYTES;
        let pcm = Pcm {
            wav: into_wav(result.stdout, data_len),
            data_len,
            truncated: data_len >= cap,
        };
        drop(held);
        Ok(pcm)
    }

    /// Sweep the root, then create the set's directory holding `input`.
    async fn open_dir(&self, input: Bytes) -> Result<JobDir, TransformError> {
        let root = self.root.clone();
        let created = tokio::task::spawn_blocking(move || -> io::Result<JobDir> {
            root.sweep(None)?;
            let dir = root.create_job_dir()?;
            dir.write_input(&input)?;
            Ok(dir)
        })
        .await;
        match created {
            Ok(Ok(dir)) => Ok(dir),
            Ok(Err(error)) => {
                tracing::warn!(kind = ?error.kind(), "media job directory could not be prepared");
                Err(TransformError::job(TransformOutcome::SandboxUnavailable))
            }
            Err(_) => {
                tracing::warn!("media job directory task failed");
                Err(TransformError::job(TransformOutcome::SandboxUnavailable))
            }
        }
    }

    /// Run one job: sweep everything but `dir`, reserve its stdout, launch,
    /// validate a clean exit's output and count the final outcome. The
    /// returned permit holds the reservation until the caller has converted
    /// the output.
    ///
    /// The sweep is synchronous: the root is a tmpfs holding at most this
    /// one directory, so it takes microseconds.
    async fn run(
        &self,
        dir: &JobDir,
        op: JobOp,
        deadline: Instant,
        (stdout_cap, stdout_headroom): (usize, usize),
        valid: impl FnOnce(&[u8]) -> bool,
    ) -> (JobResult, Option<InflightPermit>) {
        if let Err(error) = self.root.sweep(Some(dir.path())) {
            tracing::warn!(kind = ?error.kind(), "media root sweep failed before a job");
        }
        let reserve = (stdout_headroom + stdout_cap) as u64;
        let Some(held) = self.inflight.acquire(reserve, deadline).await else {
            let result = JobResult::unavailable(TransformOutcome::Timeout);
            observe(&op, &result);
            return (result, None);
        };
        let mut result = self
            .launcher
            .run(JobSpec {
                op: op.clone(),
                job_dir: dir.path().to_path_buf(),
                deadline,
                stdout_cap,
                stdout_headroom,
            })
            .await;
        if result.outcome == TransformOutcome::Ok
            && !valid(result.stdout.get(stdout_headroom..).unwrap_or_default())
        {
            result.outcome = TransformOutcome::OutputInvalid;
        }
        observe(&op, &result);
        (result, Some(held))
    }
}

impl<L: JobLauncher> MediaTransformer for FfmpegJobTransformer<L> {
    fn keyframes(
        &self,
        request: KeyframeRequest,
    ) -> BoxFuture<'_, Result<Keyframes, TransformError>> {
        Box::pin(self.keyframe_set(request))
    }

    fn audio(&self, request: AudioRequest) -> BoxFuture<'_, Result<Pcm, TransformError>> {
        Box::pin(self.audio_set(request))
    }
}

/// `wanted` positions spread evenly over `duration` seconds, at the middle of
/// each equal share, in milliseconds; one at the start when the duration is
/// unknown.
pub fn positions(duration: Option<f64>, wanted: u8) -> Vec<u64> {
    let Some(duration) = duration else {
        return vec![0];
    };
    let count = f64::from(wanted.max(1));
    (0..wanted.max(1))
        .map(|index| (duration * (f64::from(index) + 0.5) / count * 1_000.0).round() as u64)
        .collect()
}

/// A whole JPEG: the SOI marker first and the EOI marker last.
pub fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes.starts_with(&[0xFF, 0xD8]) && bytes.ends_with(&[0xFF, 0xD9])
}

/// The launcher's buffer as a WAV: the header is written into its zero
/// headroom, and the unused capacity behind the output is handed back to
/// the allocator (a shrinking `realloc`, which large allocations satisfy in
/// place) so a short voice note does not pin a ten-minute buffer while it
/// uploads.
fn into_wav(mut buffer: BytesMut, data_len: usize) -> Bytes {
    // data_len is at most 1500 s of PCM, 48 MB, far below u32::MAX.
    write_wav_header(&mut buffer, u32::try_from(data_len).unwrap_or(u32::MAX));
    let mut wav = Vec::from(buffer);
    wav.shrink_to_fit();
    Bytes::from(wav)
}

/// Remove a finished set's directory. One already removed by a later set's
/// sweep is not an error.
async fn close_dir(dir: JobDir) {
    match dir.close().await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            tracing::debug!("media job directory already swept");
        }
        Err(error) => tracing::warn!(
            kind = ?error.kind(),
            "media job directory not removed; the next sweep removes it"
        ),
    }
}
