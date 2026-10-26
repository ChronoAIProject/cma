//! The self-test's judgement over a launcher scripted by each fixture's
//! content, on every platform; the sandboxed path is covered by
//! `tests/media_sandbox/self_test.rs`.
use super::*;
use crate::{
    media::{
        budget::InflightBytes,
        transform::{
            argv::JobOp,
            ffmpeg::MAX_FRAMES,
            jobdir::{MediaRoot, INPUT},
            outcome::StderrClass,
            scheduler::TransformScheduler,
            TransformError,
        },
        MediaLimits,
    },
    MediaConfig,
};
use ::image::{codecs::jpeg::JpegEncoder, ExtendedColorType};
use std::path::PathBuf;

/// A directory removed with everything in it when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("cmaeg-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let pixels = vec![128_u8; (width * height) as usize];
    JpegEncoder::new_with_quality(&mut out, 90)
        .encode(&pixels, width, height, ExtendedColorType::L8)
        .unwrap();
    out
}

fn result(spec: &JobSpec, outcome: TransformOutcome, output: &[u8]) -> JobResult {
    let mut stdout = BytesMut::new();
    stdout.resize(spec.stdout_headroom, 0);
    stdout.extend_from_slice(&output[..output.len().min(spec.stdout_cap)]);
    JobResult {
        outcome,
        stdout,
        elapsed: Duration::from_millis(1),
        cpu: Duration::ZERO,
        stderr_class: StderrClass::None,
    }
}

/// Plays the mode named by the job's `input`: `ok` answers every op well,
/// and each other mode breaks one thing.
struct ScriptedLauncher;

impl JobLauncher for ScriptedLauncher {
    fn run(&self, spec: JobSpec) -> BoxFuture<'_, JobResult> {
        Box::pin(async move {
            let mode = std::fs::read_to_string(spec.job_dir.join(INPUT)).unwrap();
            let ok = TransformOutcome::Ok;
            match (mode.trim(), &spec.op) {
                ("probe-fails", JobOp::Probe) => result(&spec, TransformOutcome::ExitNonzero, b""),
                ("no-duration", JobOp::Probe) => result(&spec, ok, br#"{"streams":[]}"#),
                (_, JobOp::Probe) => result(&spec, ok, br#"{"format":{"duration":"2.000"}}"#),
                ("frame-cpu", JobOp::Frame(_)) => result(&spec, TransformOutcome::CpuLimit, b""),
                ("frame-fails", JobOp::Frame(_)) => {
                    result(&spec, TransformOutcome::ExitNonzero, b"")
                }
                // The first of six positions over two seconds.
                ("first-frame-fails", JobOp::Frame(time)) if time.as_str() == "0.167" => {
                    result(&spec, TransformOutcome::ExitNonzero, b"")
                }
                ("big-frame", JobOp::Frame(_)) => result(&spec, ok, &jpeg(800, 8)),
                (_, JobOp::Frame(_)) => result(&spec, ok, &jpeg(8, 8)),
                ("short-pcm", JobOp::Audio(_)) => result(&spec, ok, &[0; 2]),
                ("audio-killed", JobOp::Audio(_)) => result(&spec, TransformOutcome::Killed, b""),
                (_, JobOp::Audio(_)) => result(&spec, ok, &[0; 64_000]),
                (_, JobOp::SandboxCheck) => result(&spec, ok, b""),
            }
        })
    }
}

/// Fixtures named after every case, each holding `ok` unless `modes` names
/// another mode for it; `skip` leaves files out.
struct Harness {
    fixtures: TempDir,
    _root: TempDir,
    transformer: FfmpegJobTransformer<RecordingLauncher<ScriptedLauncher>>,
}

impl Harness {
    fn new(modes: &[(&str, &str)], skip: &[&str]) -> Self {
        let fixtures = TempDir::new("self-test-fixtures");
        for case in CASES.iter().filter(|case| !skip.contains(&case.file)) {
            let mode = modes
                .iter()
                .find(|(file, _)| *file == case.file)
                .map_or("ok", |(_, mode)| mode);
            std::fs::write(fixtures.0.join(case.file), mode).unwrap();
        }
        let root = TempDir::new("self-test-root");
        let transformer = FfmpegJobTransformer::new(
            RecordingLauncher::new(ScriptedLauncher),
            MediaRoot::new(&root.0),
            TransformScheduler::new(),
            InflightBytes::new(&MediaLimits::from_config(&MediaConfig::default())),
            MAX_FRAMES,
        );
        Self {
            fixtures,
            _root: root,
            transformer,
        }
    }

    async fn report(&self) -> Report {
        run(
            &self.transformer,
            &self.fixtures.0,
            &Arc::new(Semaphore::new(1)),
        )
        .await
    }
}

fn outcome_of(report: &Report, file: &str, op: CaseOp) -> CaseOutcome {
    report
        .lines
        .iter()
        .find(|line| line.file == file && line.op == op)
        .unwrap_or_else(|| panic!("no {file} {op:?} line"))
        .outcome
}

#[tokio::test]
async fn every_case_passes_with_well_formed_outputs() {
    let report = Harness::new(&[], &[]).report().await;
    assert!(report.passed(), "{report:?}");
    let ops: Vec<_> = report
        .lines
        .iter()
        .map(|line| (line.file, line.op))
        .collect();
    assert_eq!(
        ops,
        [
            ("h264_aac.mp4", CaseOp::Keyframes),
            ("h264_aac.mp4", CaseOp::Audio),
            ("vp9_opus.webm", CaseOp::Keyframes),
            ("vp9_opus.webm", CaseOp::Audio),
            ("voice.ogg", CaseOp::Audio),
            ("tone.mp3", CaseOp::Audio),
            ("tone.amr", CaseOp::Audio),
            ("anim.gif", CaseOp::Keyframes),
            ("hevc_4k.mp4", CaseOp::Keyframes),
        ]
    );
    let hevc = &report.lines[8];
    assert_eq!(hevc.frames, 1);
    assert_eq!(report.lines[0].frames, 6);
    assert_eq!(report.lines[4].pcm_bytes, 64_000);
}

#[tokio::test]
async fn a_report_line_carries_counts_and_never_media() {
    let report = Harness::new(&[], &[]).report().await;
    let line = report.lines[0].to_string();
    let (head, ms) = line.rsplit_once(" ms=").unwrap();
    assert_eq!(
        head,
        "media-self-test case=h264_aac.mp4 op=keyframes outcome=ok frames=6 pcm_bytes=0"
    );
    assert!(ms.parse::<u128>().is_ok(), "{line}");
}

#[tokio::test]
async fn a_skipped_frame_passes_unless_the_case_is_exact() {
    let report = Harness::new(
        &[
            ("h264_aac.mp4", "first-frame-fails"),
            ("hevc_4k.mp4", "frame-fails"),
        ],
        &[],
    )
    .report()
    .await;
    let h264 = &report.lines[0];
    assert_eq!((h264.outcome, h264.frames), (CaseOutcome::Ok, 5));
    assert_eq!(
        outcome_of(&report, "hevc_4k.mp4", CaseOp::Keyframes),
        CaseOutcome::Job(TransformOutcome::ExitNonzero)
    );
    assert!(!report.passed());
}

#[tokio::test]
async fn a_limit_on_any_job_fails_the_case() {
    let report = Harness::new(
        &[("anim.gif", "frame-cpu"), ("tone.mp3", "audio-killed")],
        &[],
    )
    .report()
    .await;
    assert_eq!(
        outcome_of(&report, "anim.gif", CaseOp::Keyframes),
        CaseOutcome::Job(TransformOutcome::CpuLimit)
    );
    assert_eq!(
        outcome_of(&report, "tone.mp3", CaseOp::Audio),
        CaseOutcome::Job(TransformOutcome::Killed)
    );
    assert!(!report.passed());
}

#[tokio::test]
async fn a_failed_probe_fails_the_keyframe_case() {
    let report = Harness::new(
        &[
            ("h264_aac.mp4", "probe-fails"),
            ("vp9_opus.webm", "no-duration"),
        ],
        &[],
    )
    .report()
    .await;
    assert_eq!(
        outcome_of(&report, "h264_aac.mp4", CaseOp::Keyframes),
        CaseOutcome::Job(TransformOutcome::ExitNonzero)
    );
    assert_eq!(
        outcome_of(&report, "vp9_opus.webm", CaseOp::Keyframes),
        CaseOutcome::ProbeInvalid
    );
    // Audio does not depend on the probe.
    assert_eq!(
        outcome_of(&report, "h264_aac.mp4", CaseOp::Audio),
        CaseOutcome::Ok
    );
}

#[tokio::test]
async fn an_oversized_frame_and_short_pcm_fail() {
    let report = Harness::new(
        &[("anim.gif", "big-frame"), ("voice.ogg", "short-pcm")],
        &[],
    )
    .report()
    .await;
    assert_eq!(
        outcome_of(&report, "anim.gif", CaseOp::Keyframes),
        CaseOutcome::FrameInvalid
    );
    let voice = report
        .lines
        .iter()
        .find(|line| line.file == "voice.ogg")
        .unwrap();
    assert_eq!(
        (voice.outcome, voice.pcm_bytes),
        (CaseOutcome::PcmInvalid, 2)
    );
    // Two bytes are enough where no minimum is set.
    let short = Harness::new(&[("tone.mp3", "short-pcm")], &[])
        .report()
        .await;
    assert!(short.passed(), "{short:?}");
}

#[tokio::test]
async fn a_missing_fixture_fails_its_case() {
    let report = Harness::new(&[], &["tone.amr"]).report().await;
    assert_eq!(
        outcome_of(&report, "tone.amr", CaseOp::Audio),
        CaseOutcome::FixtureUnreadable
    );
    assert!(!report.passed());
}

#[test]
fn pcm_is_judged_on_parity_length_and_header() {
    let pcm = |data_len: usize, header_len: u32| {
        let mut wav = BytesMut::zeroed(WAV_HEADER_BYTES + data_len);
        write_wav_header(&mut wav, header_len);
        Pcm {
            wav: wav.freeze(),
            data_len,
            truncated: false,
        }
    };
    assert_eq!(judge_pcm(&pcm(64_000, 64_000), 64_000), CaseOutcome::Ok);
    assert_eq!(judge_pcm(&pcm(2, 2), 0), CaseOutcome::Ok);
    assert_eq!(judge_pcm(&pcm(0, 0), 0), CaseOutcome::PcmInvalid);
    assert_eq!(judge_pcm(&pcm(3, 3), 0), CaseOutcome::PcmInvalid);
    assert_eq!(
        judge_pcm(&pcm(62_000, 62_000), 64_000),
        CaseOutcome::PcmInvalid
    );
    assert_eq!(judge_pcm(&pcm(4, 6), 0), CaseOutcome::PcmInvalid);
}

#[test]
fn an_empty_report_does_not_pass() {
    assert!(!Report { lines: Vec::new() }.passed());
}

#[test]
fn every_case_asks_for_something_and_names_a_distinct_file() {
    let mut files: Vec<_> = CASES.iter().map(|case| case.file).collect();
    files.sort_unstable();
    files.dedup();
    assert_eq!(files.len(), CASES.len());
    assert!(CASES
        .iter()
        .all(|case| case.keyframes.is_some() || case.audio_seconds.is_some()));
    assert!(CASES.iter().all(|case| case
        .keyframes
        .is_none_or(|frames| (1..=MAX_FRAMES).contains(&frames))));
}

#[tokio::test]
async fn the_recording_launcher_drains_its_record_in_order() {
    let root = TempDir::new("self-test-record");
    let dir = MediaRoot::new(&root.0).create_job_dir().unwrap();
    dir.write_input(&Bytes::from_static(b"frame-fails"))
        .unwrap();
    let launcher = RecordingLauncher::new(ScriptedLauncher);
    for op in [JobOp::Probe, JobOp::SandboxCheck] {
        launcher
            .run(JobSpec {
                op,
                job_dir: dir.path().to_path_buf(),
                deadline: Instant::now() + Duration::from_secs(1),
                stdout_cap: 1024,
                stdout_headroom: 0,
            })
            .await;
    }
    let ok = TransformOutcome::Ok;
    assert_eq!(
        launcher.take(),
        [
            JobRecord {
                op: "probe",
                outcome: ok
            },
            JobRecord {
                op: "sandbox_check",
                outcome: ok
            }
        ]
    );
    assert!(launcher.take().is_empty());
}

#[test]
fn outcomes_name_every_failure() {
    assert_eq!(
        CaseOutcome::of(TransformError::job(TransformOutcome::OutputInvalid)),
        CaseOutcome::Job(TransformOutcome::OutputInvalid)
    );
    assert_eq!(
        CaseOutcome::of(TransformError::NoFrames),
        CaseOutcome::NoFrames
    );
    assert_eq!(
        CaseOutcome::of(TransformError::Unavailable),
        CaseOutcome::Unavailable
    );
    let names: Vec<_> = [
        CaseOutcome::Ok,
        CaseOutcome::Job(TransformOutcome::Timeout),
        CaseOutcome::NoFrames,
        CaseOutcome::Unavailable,
        CaseOutcome::ProbeInvalid,
        CaseOutcome::FrameCount,
        CaseOutcome::FrameInvalid,
        CaseOutcome::PcmInvalid,
        CaseOutcome::FixtureUnreadable,
    ]
    .iter()
    .map(CaseOutcome::as_str)
    .collect();
    assert_eq!(
        names,
        [
            "ok",
            "timeout",
            "no_frames",
            "unavailable",
            "probe_invalid",
            "frame_count",
            "frame_invalid",
            "pcm_invalid",
            "fixture_unreadable"
        ]
    );
}
