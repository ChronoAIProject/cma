//! The transformer's startup sequence, run before the listener binds and
//! only when the transformer is configured.
//!
//! It validates the binary directory, checks and sweeps the media root,
//! opens the container's cgroup memory files, starts the watchdog thread
//! (so later thread exhaustion cannot stop it) and runs the sandbox
//! self-check through the production launcher, exactly as a job runs:
//! env-cleared, in its own process group, behind a pidfd and a watchdog
//! registration. Any failure fails startup; a refused startup loses no
//! callbacks because `maxUnavailable: 0` keeps the old pod serving.
use super::{jobdir::MediaRoot, launcher::JobLauncher, watchdog::MemoryGauge};
use crate::{Config, ConfigError};
use std::path::PathBuf;

/// The container's cgroup v2 directory.
pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// What a configured transformer runs on once setup passed.
pub struct TransformerRuntime {
    pub launcher: Box<dyn JobLauncher>,
    pub root: MediaRoot,
}

/// Where the watchdog's gauge comes from.
pub enum GaugeSource {
    /// The cgroup v2 directory to read, `/sys/fs/cgroup` in production.
    Cgroup(PathBuf),
    /// A ready gauge, for tests.
    Given(Box<dyn MemoryGauge>),
}

/// Everything setup needs, injected so tests use temporary directories, the
/// test build's binaries and a fake gauge.
pub struct Setup {
    pub ffmpeg_dir: PathBuf,
    /// The owner the binary directory must have: root in production.
    pub owner: u32,
    pub root: MediaRoot,
    pub gauge: GaugeSource,
    /// The binary each job executes: `current_exe()` in production.
    pub job_exe: PathBuf,
}

/// The production setup when `config` configures the transformer; `None`
/// (and `cmaeg_media_sandbox_ready` left at 0) otherwise.
pub async fn start_configured(config: &Config) -> Result<Option<TransformerRuntime>, ConfigError> {
    let Some(ffmpeg_dir) = config
        .transformer
        .ffmpeg_dir
        .clone()
        .filter(|_| config.transformer.configured(&config.media))
    else {
        return Ok(None);
    };
    let job_exe = std::env::current_exe().map_err(|error| {
        unavailable(format!(
            "the gateway binary's path is unknown ({:?})",
            error.kind()
        ))
    })?;
    start(Setup {
        ffmpeg_dir,
        owner: 0,
        root: MediaRoot::new(super::jobdir::MEDIA_ROOT),
        gauge: GaugeSource::Cgroup(PathBuf::from(CGROUP_ROOT)),
        job_exe,
    })
    .await
    .map(Some)
}

fn unavailable(detail: String) -> ConfigError {
    ConfigError::MediaTransformer { detail }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use linux::start;

/// Where no sandbox exists, a configured transformer is a startup error.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub async fn start(_setup: Setup) -> Result<TransformerRuntime, ConfigError> {
    Err(unavailable(
        "the media job sandbox requires linux x86_64".into(),
    ))
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux {
    use super::{unavailable, GaugeSource, Setup, TransformerRuntime};
    use crate::{
        media::transform::{
            argv::JobOp,
            bin_dir::FfmpegDir,
            launcher::{observe, JobLauncher, JobSpec, ProcessLauncher},
            outcome::TransformOutcome,
            watchdog::{CgroupV2Gauge, MemoryGauge, PidfdSignaller, Watchdog},
        },
        telemetry::names,
        ConfigError, CMAEG_MEDIA_FFMPEG_DIR,
    };

    /// Run the startup sequence; see the module documentation.
    pub async fn start(setup: Setup) -> Result<TransformerRuntime, ConfigError> {
        let dir = FfmpegDir::validate(&setup.ffmpeg_dir, setup.owner).map_err(|error| {
            ConfigError::InvalidMedia {
                detail: format!("{CMAEG_MEDIA_FFMPEG_DIR} is unusable: {error}"),
            }
        })?;
        let root = setup.root;
        root.check()
            .and_then(|()| root.sweep(None))
            .map_err(|error| {
                unavailable(format!("the media root is unusable ({:?})", error.kind()))
            })?;
        let gauge: Box<dyn MemoryGauge> = match setup.gauge {
            GaugeSource::Cgroup(path) => Box::new(CgroupV2Gauge::open(&path).map_err(|error| {
                unavailable(format!("the cgroup memory files are unusable: {error}"))
            })?),
            GaugeSource::Given(gauge) => gauge,
        };
        let watchdog = Watchdog::start(gauge, Box::new(PidfdSignaller)).map_err(|error| {
            unavailable(format!(
                "the media watchdog thread did not start ({:?})",
                error.kind()
            ))
        })?;
        let (memory_max_bytes, watchdog_threshold_bytes) = (watchdog.limit(), watchdog.threshold());
        let launcher =
            ProcessLauncher::new(setup.job_exe, dir.path().to_path_buf(), Some(watchdog));
        let outcome = self_check(&launcher, &root).await?;
        if outcome != TransformOutcome::Ok {
            return Err(unavailable(format!(
                "media sandbox self-check failed: {outcome}"
            )));
        }
        metrics::gauge!(names::MEDIA_SANDBOX_READY).set(1);
        tracing::info!(
            dumpable = false,
            sandbox_check = "passed",
            memory_max_bytes,
            watchdog_threshold_bytes,
            "media transformer ready"
        );
        Ok(TransformerRuntime {
            launcher: Box::new(launcher),
            root,
        })
    }

    /// `cmaeg --media-sandbox-check` through the launcher, in a fresh job
    /// directory that is only its working directory.
    async fn self_check(
        launcher: &ProcessLauncher,
        root: &crate::media::transform::jobdir::MediaRoot,
    ) -> Result<TransformOutcome, ConfigError> {
        let dir = root.create_job_dir().map_err(|error| {
            unavailable(format!(
                "the self-check's job directory could not be created ({:?})",
                error.kind()
            ))
        })?;
        let result = launcher
            .run(JobSpec::sandbox_check(dir.path().to_path_buf()))
            .await;
        observe(&JobOp::SandboxCheck, &result);
        if let Err(error) = dir.close().await {
            tracing::warn!(
                kind = ?error.kind(),
                "the self-check's job directory was not removed; the next sweep removes it"
            );
        }
        Ok(result.outcome)
    }
}
