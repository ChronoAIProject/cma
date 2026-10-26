//! Keyframe and audio sets over a scripted job launcher, in paused time:
//! frame positions, deadlines, skipped frames, PCM output and the in-flight
//! byte reservations.
use super::budget::channel;
use bytes::BytesMut;
use cmaeg::{
    media::{
        budget::InflightBytes,
        transform::{
            argv::JobOp,
            ffmpeg::{positions, FfmpegJobTransformer},
            jobdir::{MediaRoot, INPUT},
            launcher::{JobLauncher, JobResult, JobSpec, JOB_WALL},
            outcome::{StderrClass, TransformOutcome},
            AudioRequest, KeyframeRequest, MediaTransformer, TransformError,
        },
        MediaLimits,
    },
    MediaConfig,
};
use futures_util::future::BoxFuture;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::{sleep_until, Instant};

/// What the fake launcher does for one job.
#[derive(Debug, Clone)]
pub(super) struct Step {
    pub runs_for: Duration,
    pub outcome: TransformOutcome,
    pub stdout: Vec<u8>,
    pub stderr_class: StderrClass,
}

impl Step {
    pub fn ok(runs_for: Duration, stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            runs_for,
            outcome: TransformOutcome::Ok,
            stdout: stdout.into(),
            stderr_class: StderrClass::None,
        }
    }

    pub fn fail(outcome: TransformOutcome) -> Self {
        Self {
            runs_for: Duration::from_millis(100),
            outcome,
            stdout: Vec::new(),
            stderr_class: StderrClass::Other,
        }
    }
}

/// What the fake launcher saw of one job.
#[derive(Debug, Clone)]
pub(super) struct Ran {
    pub op: JobOp,
    pub input: Vec<u8>,
    pub started: Instant,
    pub deadline: Instant,
    pub cap: usize,
    pub headroom: usize,
    /// The pod's in-flight bytes still free when the job was spawned.
    pub inflight_free: u64,
}

type Script = dyn Fn(&JobSpec) -> Step + Send + Sync;

/// Plays a [`Step`] per job, honouring the per-process deadline the real
/// launcher enforces, and records every job.
pub(super) struct FakeLauncher {
    script: Box<Script>,
    ran: Arc<Mutex<Vec<Ran>>>,
    inflight: InflightBytes,
}

impl JobLauncher for FakeLauncher {
    fn run(&self, spec: JobSpec) -> BoxFuture<'_, JobResult> {
        Box::pin(async move {
            let started = Instant::now();
            let step = (self.script)(&spec);
            self.ran.lock().unwrap().push(Ran {
                op: spec.op.clone(),
                input: std::fs::read(spec.job_dir.join(INPUT)).unwrap(),
                started,
                deadline: spec.deadline,
                cap: spec.stdout_cap,
                headroom: spec.stdout_headroom,
                inflight_free: self.inflight.available_bytes(),
            });
            let mut stdout = BytesMut::new();
            stdout.resize(spec.stdout_headroom, 0);
            let deadline = spec.effective_deadline(started);
            if started + step.runs_for > deadline {
                sleep_until(deadline).await;
                return JobResult {
                    outcome: TransformOutcome::Timeout,
                    stdout,
                    elapsed: deadline - started,
                    cpu: Duration::ZERO,
                    stderr_class: StderrClass::None,
                };
            }
            sleep_until(started + step.runs_for).await;
            let kept = step.stdout.len().min(spec.stdout_cap);
            stdout.extend_from_slice(&step.stdout[..kept]);
            JobResult {
                outcome: step.outcome,
                stdout,
                elapsed: step.runs_for,
                cpu: Duration::ZERO,
                stderr_class: step.stderr_class,
            }
        })
    }
}

/// A transformer over a [`FakeLauncher`] and a temporary media root.
pub(super) struct Harness {
    pub transformer: Arc<FfmpegJobTransformer<FakeLauncher>>,
    pub ran: Arc<Mutex<Vec<Ran>>>,
    pub inflight: InflightBytes,
    root: PathBuf,
}

impl Harness {
    pub fn new(script: impl Fn(&JobSpec) -> Step + Send + Sync + 'static) -> Self {
        let root = std::env::temp_dir().join(format!("cmaeg-transform-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let inflight = InflightBytes::new(&MediaLimits::from_config(&MediaConfig::default()));
        let ran = Arc::new(Mutex::new(Vec::new()));
        let launcher = FakeLauncher {
            script: Box::new(script),
            ran: ran.clone(),
            inflight: inflight.clone(),
        };
        let transformer = FfmpegJobTransformer::new(
            launcher,
            MediaRoot::new(&root),
            Default::default(),
            inflight.clone(),
            6,
        );
        Self {
            transformer: Arc::new(transformer),
            ran,
            inflight,
            root,
        }
    }

    pub fn ran(&self) -> Vec<Ran> {
        self.ran.lock().unwrap().clone()
    }

    /// The op and argument of every job, as `probe`, `frame 0.833`, `audio 600`.
    pub fn ops(&self) -> Vec<String> {
        self.ran().iter().map(|ran| describe(&ran.op)).collect()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(super) fn describe(op: &JobOp) -> String {
    match op {
        JobOp::Probe => "probe".into(),
        JobOp::Frame(time) => format!("frame {}", time.as_str()),
        JobOp::Audio(seconds) => format!("audio {}", seconds.get()),
        JobOp::SandboxCheck => "sandbox_check".into(),
    }
}

pub(super) fn jpeg(marker: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, marker, marker, 0xFF, 0xD9]
}

pub(super) fn probe_json(duration: &str) -> String {
    format!(
        r#"{{"programs":[],"stream_groups":[],"streams":[{{"codec_type":"video"}}],"format":{{"duration":"{duration}"}}}}"#
    )
}

pub(super) const SECOND: Duration = Duration::from_secs(1);

/// A probe of `duration` seconds, then every frame `runs_for`.
pub(super) fn video(duration: &'static str, runs_for: Duration) -> impl Fn(&JobSpec) -> Step {
    move |spec| match &spec.op {
        JobOp::Probe => Step::ok(SECOND, probe_json(duration)),
        _ => Step::ok(runs_for, jpeg(1)),
    }
}

pub(super) fn keyframes(name: &str, frames: u8) -> KeyframeRequest {
    KeyframeRequest {
        channel: channel(name),
        input: name.as_bytes().to_vec().into(),
        frames,
    }
}

pub(super) fn audio(name: &str, input: &str, seconds: u16) -> AudioRequest {
    AudioRequest {
        channel: channel(name),
        input: input.as_bytes().to_vec().into(),
        seconds,
    }
}

#[test]
fn keyframe_times_are_spread() {
    assert_eq!(
        positions(Some(10.0), 6),
        [833, 2_500, 4_167, 5_833, 7_500, 9_167]
    );
}

#[tokio::test(start_paused = true)]
async fn keyframe_times_are_formatted_with_three_decimals() {
    let harness = Harness::new(video("10.000000", SECOND));
    let set = harness
        .transformer
        .keyframes(keyframes("video", 6))
        .await
        .unwrap();
    assert_eq!(
        harness.ops(),
        [
            "probe",
            "frame 0.833",
            "frame 2.500",
            "frame 4.167",
            "frame 5.833",
            "frame 7.500",
            "frame 9.167"
        ]
    );
    let at: Vec<u64> = set.frames.iter().map(|(_, at_ms)| *at_ms).collect();
    assert_eq!(at, [833, 2_500, 4_167, 5_833, 7_500, 9_167]);
    assert_eq!(set.duration_ms, Some(10_000));
    assert!(!set.partial);
    assert!(harness.ran().iter().all(|ran| ran.input == b"video"));
}

#[tokio::test(start_paused = true)]
async fn unknown_duration_takes_one_frame_at_zero() {
    assert_eq!(positions(None, 6), [0]);
    for probe in [
        Step::fail(TransformOutcome::ExitNonzero),
        Step::fail(TransformOutcome::OutputInvalid),
        Step::ok(SECOND, "not json"),
        Step::ok(SECOND, probe_json("N/A")),
    ] {
        let harness = Harness::new(move |spec| match spec.op {
            JobOp::Probe => probe.clone(),
            _ => Step::ok(SECOND, jpeg(2)),
        });
        let set = harness
            .transformer
            .keyframes(keyframes("gif", 6))
            .await
            .unwrap();
        assert_eq!(harness.ops(), ["probe", "frame 0.000"]);
        assert_eq!(set.frames.len(), 1);
        assert_eq!(set.frames[0].1, 0);
        assert_eq!(set.duration_ms, None);
        assert!(!set.partial);
    }
}

#[tokio::test(start_paused = true)]
async fn any_other_probe_failure_ends_the_set() {
    let harness = Harness::new(|_: &JobSpec| Step::fail(TransformOutcome::ResourceLimit));
    let error = harness
        .transformer
        .keyframes(keyframes("video", 6))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        TransformError::Job {
            outcome: TransformOutcome::ResourceLimit,
            stderr_class: StderrClass::Other
        }
    );
    assert_eq!(harness.ops(), ["probe"]);
}

#[tokio::test(start_paused = true)]
async fn per_process_deadline_is_20s() {
    assert_eq!(JOB_WALL, Duration::from_secs(20));
    let harness = Harness::new(|spec: &JobSpec| match &spec.op {
        JobOp::Probe => Step::ok(Duration::from_secs(25), probe_json("10")),
        _ => Step::ok(SECOND, jpeg(3)),
    });
    let started = Instant::now();
    let error = harness
        .transformer
        .keyframes(keyframes("video", 2))
        .await
        .unwrap_err();
    // The probe runs under the set's 30 s deadline but is stopped at 20 s,
    // and a probe that timed out ends its set.
    assert_eq!(harness.ran()[0].deadline, started + Duration::from_secs(30));
    assert_eq!(Instant::now(), started + Duration::from_secs(20));
    assert_eq!(error, TransformError::job(TransformOutcome::Timeout));
    assert_eq!(harness.ops(), ["probe"]);
    let spec = JobSpec {
        op: JobOp::Probe,
        job_dir: PathBuf::from("/"),
        deadline: started + Duration::from_secs(30),
        stdout_cap: 0,
        stdout_headroom: 0,
    };
    assert_eq!(
        spec.effective_deadline(started),
        started + Duration::from_secs(20)
    );
    assert_eq!(
        spec.effective_deadline(started + Duration::from_secs(15)),
        started + Duration::from_secs(30)
    );
}

#[tokio::test(start_paused = true)]
async fn keyframe_set_deadline_is_30s_including_slot_wait() {
    let harness = Harness::new(|spec: &JobSpec| match &spec.op {
        JobOp::Audio(_) => Step::ok(Duration::from_secs(10), vec![0; 64]),
        JobOp::Probe => Step::ok(SECOND, probe_json("4")),
        _ => Step::ok(SECOND, jpeg(4)),
    });
    let started = Instant::now();
    let transformer = harness.transformer.clone();
    let voice = tokio::spawn(async move { transformer.audio(audio("a", "voice", 5)).await });
    tokio::time::sleep(Duration::from_millis(1)).await;
    let set = harness
        .transformer
        .keyframes(keyframes("b", 1))
        .await
        .unwrap();
    voice.await.unwrap().unwrap();
    let ran = harness.ran();
    assert_eq!(ran[1].op, JobOp::Probe);
    assert_eq!(ran[1].started, started + Duration::from_secs(10));
    assert_eq!(
        ran[1].deadline,
        started + Duration::from_millis(1) + Duration::from_secs(30)
    );
    assert_eq!(set.frames.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn partial_frames_returned_at_set_deadline() {
    // The probe takes 1 s and each frame 7 s: four frames end by 29 s and
    // the fifth is stopped at the set's 30 s deadline.
    let harness = Harness::new(video("10", Duration::from_secs(7)));
    let started = Instant::now();
    let set = harness
        .transformer
        .keyframes(keyframes("video", 6))
        .await
        .unwrap();
    assert_eq!(Instant::now(), started + Duration::from_secs(30));
    assert_eq!(set.frames.len(), 4);
    assert!(set.partial);
    assert_eq!(harness.ops().len(), 6);
}

#[tokio::test(start_paused = true)]
async fn zero_frames_is_timed_out() {
    // The first frame is stopped by its own 20 s wall, the second by the
    // set's deadline: the deadline, not a frame failure, ended the set.
    let harness = Harness::new(video("10", Duration::from_secs(40)));
    let error = harness
        .transformer
        .keyframes(keyframes("video", 6))
        .await
        .unwrap_err();
    assert_eq!(error, TransformError::NoFrames);
    assert_eq!(harness.ops().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn failed_frame_is_skipped_and_set_continues() {
    let harness = Harness::new(|spec: &JobSpec| match &spec.op {
        JobOp::Probe => Step::ok(SECOND, probe_json("10")),
        JobOp::Frame(time) if time.as_str() == "4.167" => Step::fail(TransformOutcome::ExitNonzero),
        JobOp::Frame(time) if time.as_str() == "7.500" => Step::ok(SECOND, b"not a jpeg".to_vec()),
        _ => Step::ok(SECOND, jpeg(5)),
    });
    let set = harness
        .transformer
        .keyframes(keyframes("video", 6))
        .await
        .unwrap();
    assert_eq!(harness.ops().len(), 7, "one probe and six frame jobs");
    let at: Vec<u64> = set.frames.iter().map(|(_, at_ms)| *at_ms).collect();
    assert_eq!(at, [833, 2_500, 5_833, 9_167]);
    assert!(set.partial);
    assert!(set.frames.iter().all(|(frame, _)| frame[..] == jpeg(5)[..]));
}

#[tokio::test(start_paused = true)]
async fn all_frames_failing_returns_the_job_error() {
    let harness = Harness::new(|spec: &JobSpec| match &spec.op {
        JobOp::Probe => Step::ok(SECOND, probe_json("10")),
        _ => Step {
            stderr_class: StderrClass::UnsupportedCodec,
            ..Step::fail(TransformOutcome::ExitNonzero)
        },
    });
    let error = harness
        .transformer
        .keyframes(keyframes("video", 3))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        TransformError::Job {
            outcome: TransformOutcome::ExitNonzero,
            stderr_class: StderrClass::UnsupportedCodec
        }
    );
    assert_eq!(harness.ops().len(), 4);
}
