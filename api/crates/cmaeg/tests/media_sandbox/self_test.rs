//! The build-time self-test through the real launcher and sandbox, with the
//! probe standing in for FFmpeg, and its command line.
use super::support::{bin_dir, TempDir};
use cmaeg::media::{
    budget::InflightBytes,
    transform::{
        bin_dir::FfmpegDir,
        ffmpeg::{FfmpegJobTransformer, MAX_FRAMES},
        jobdir::MediaRoot,
        launcher::ProcessLauncher,
        outcome::TransformOutcome,
        scheduler::TransformScheduler,
        self_test::{run, CaseOp, CaseOutcome, RecordingLauncher, CASES},
        watchdog::{DisabledGauge, PidfdSignaller, Watchdog},
    },
    MediaLimits,
};
use std::{process::Command, sync::Arc};
use tokio::sync::Semaphore;

struct Fixture {
    bin: TempDir,
    root: TempDir,
    fixtures: TempDir,
}

impl Fixture {
    /// Every case's fixture holds `media` unless `broken` names it.
    fn new(broken: Option<&str>) -> Self {
        let fixtures = TempDir::new("media-fixtures", 0o755);
        for case in CASES {
            let mode = if broken == Some(case.file) {
                "broken"
            } else {
                "media"
            };
            std::fs::write(fixtures.path().join(case.file), format!("{mode}\n")).unwrap();
        }
        Self {
            bin: bin_dir(),
            root: TempDir::new("media-root", 0o700),
            fixtures,
        }
    }

    fn transformer(&self) -> FfmpegJobTransformer<RecordingLauncher<ProcessLauncher>> {
        // SAFETY: getuid has no preconditions.
        let dir = FfmpegDir::validate(self.bin.path(), unsafe { libc::getuid() }).unwrap();
        let watchdog = Watchdog::start(Box::new(DisabledGauge), Box::new(PidfdSignaller)).unwrap();
        FfmpegJobTransformer::new(
            RecordingLauncher::new(ProcessLauncher::new(
                env!("CARGO_BIN_EXE_cmaeg").into(),
                dir.path().to_path_buf(),
                Some(watchdog),
            )),
            MediaRoot::new(self.root.path()),
            TransformScheduler::new(),
            InflightBytes::new(&MediaLimits::from_config(&cmaeg::MediaConfig::default())),
            MAX_FRAMES,
        )
    }
}

#[tokio::test]
async fn self_test_passes_with_well_formed_outputs() {
    let fixture = Fixture::new(None);
    let report = run(
        &fixture.transformer(),
        fixture.fixtures.path(),
        &Arc::new(Semaphore::new(1)),
    )
    .await;
    assert!(report.passed(), "{report:?}");
    assert_eq!(report.lines.len(), 9);
    for line in &report.lines {
        assert_eq!(line.outcome, CaseOutcome::Ok, "{line}");
        match line.op {
            CaseOp::Keyframes => assert!(line.frames >= 1, "{line}"),
            CaseOp::Audio => assert_eq!(line.pcm_bytes, 64_000, "{line}"),
        }
    }
}

#[tokio::test]
async fn self_test_fails_a_broken_case() {
    let fixture = Fixture::new(Some("h264_aac.mp4"));
    let report = run(
        &fixture.transformer(),
        fixture.fixtures.path(),
        &Arc::new(Semaphore::new(1)),
    )
    .await;
    assert!(!report.passed());
    for line in &report.lines {
        let expected = if line.file == "h264_aac.mp4" {
            CaseOutcome::Job(TransformOutcome::ExitNonzero)
        } else {
            CaseOutcome::Ok
        };
        assert_eq!(line.outcome, expected, "{line}");
    }
}

#[test]
fn self_test_cli_requires_the_ffmpeg_dir() {
    let fixtures = TempDir::new("media-fixtures", 0o755);
    let output = Command::new(env!("CARGO_BIN_EXE_cmaeg"))
        .arg("--media-self-test")
        .arg(fixtures.path())
        .env_clear()
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("CMAEG_MEDIA_FFMPEG_DIR is required"));
}

/// A probe directory the test's own uid owns fails the root-owner check; run
/// as root, the missing media root fails instead. Both are configuration.
#[test]
fn self_test_cli_exits_two_on_an_unusable_configuration() {
    let fixtures = TempDir::new("media-fixtures", 0o755);
    let bin = bin_dir();
    let output = Command::new(env!("CARGO_BIN_EXE_cmaeg"))
        .arg("--media-self-test")
        .arg(fixtures.path())
        .env_clear()
        .env(cmaeg::CMAEG_MEDIA_FFMPEG_DIR, bin.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}
