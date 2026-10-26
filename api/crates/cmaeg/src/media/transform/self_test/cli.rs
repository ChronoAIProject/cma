//! The self-test's command line on Linux x86_64: it assembles the production
//! launcher exactly as gateway startup does, except that the watchdog's gauge
//! is disabled, because a build container has no memory limit to read.
use super::{run, RecordingLauncher, EXIT_FAILED, EXIT_PASSED, EXIT_USAGE};
use crate::{
    media::{
        budget::InflightBytes,
        transform::{
            bin_dir::FfmpegDir,
            ffmpeg::{FfmpegJobTransformer, MAX_FRAMES},
            jobdir::{MediaRoot, MEDIA_ROOT},
            launcher::ProcessLauncher,
            scheduler::TransformScheduler,
            watchdog::{DisabledGauge, PidfdSignaller, Watchdog},
        },
        MediaLimits,
    },
    MediaConfig, CMAEG_MEDIA_FFMPEG_DIR,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Semaphore;

/// Run the self-test and return its exit status.
pub fn status(fixtures: &Path) -> i32 {
    let transformer = match prepare(fixtures) {
        Ok(transformer) => transformer,
        Err(detail) => {
            eprintln!("cmaeg media-self-test: {detail}");
            return EXIT_USAGE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("cmaeg media-self-test: no runtime ({:?})", error.kind());
            return EXIT_USAGE;
        }
    };
    let decode = Arc::new(Semaphore::new(1));
    let report = runtime.block_on(run(&transformer, fixtures, &decode));
    let mut stdout = std::io::stdout().lock();
    let written = report
        .lines
        .iter()
        .try_for_each(|line| writeln!(stdout, "{line}"))
        .and_then(|()| stdout.flush());
    if written.is_err() {
        eprintln!("cmaeg media-self-test: stdout unavailable");
        return EXIT_FAILED;
    }
    if report.passed() {
        EXIT_PASSED
    } else {
        EXIT_FAILED
    }
}

/// The transformer the production startup would build, over the fixed
/// media root and the root-owned binary directory the key names.
fn prepare(
    fixtures: &Path,
) -> Result<FfmpegJobTransformer<RecordingLauncher<ProcessLauncher>>, String> {
    if !fixtures.is_dir() {
        return Err("the fixtures path is not a directory".into());
    }
    let dir = std::env::var_os(CMAEG_MEDIA_FFMPEG_DIR)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| format!("{CMAEG_MEDIA_FFMPEG_DIR} is required"))?;
    let dir = FfmpegDir::validate(&dir, 0)
        .map_err(|error| format!("{CMAEG_MEDIA_FFMPEG_DIR} is unusable: {error}"))?;
    let root = MediaRoot::new(MEDIA_ROOT);
    root.check()
        .and_then(|()| root.sweep(None))
        .map_err(|error| format!("the media root is unusable ({:?})", error.kind()))?;
    let job_exe = std::env::current_exe()
        .map_err(|error| format!("the gateway binary's path is unknown ({:?})", error.kind()))?;
    let watchdog =
        Watchdog::start(Box::new(DisabledGauge), Box::new(PidfdSignaller)).map_err(|error| {
            format!(
                "the media watchdog thread did not start ({:?})",
                error.kind()
            )
        })?;
    let launcher = ProcessLauncher::new(job_exe, dir.path().to_path_buf(), Some(watchdog));
    Ok(FfmpegJobTransformer::new(
        RecordingLauncher::new(launcher),
        root,
        TransformScheduler::new(),
        InflightBytes::new(&MediaLimits::from_config(&MediaConfig::default())),
        MAX_FRAMES,
    ))
}
