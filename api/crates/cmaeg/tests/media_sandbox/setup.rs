//! The transformer's startup sequence with the real launcher and sandbox.
use super::support::{bin_dir, TempDir};
use cmaeg::media::transform::{
    jobdir::MediaRoot,
    setup::{start, GaugeSource, Setup},
    watchdog::DisabledGauge,
};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

fn setup(bin: &Path, root: &Path) -> Setup {
    Setup {
        ffmpeg_dir: bin.to_path_buf(),
        // Production expects root; the probe directory is the test's own.
        owner: fs::metadata(bin).unwrap().uid(),
        root: MediaRoot::new(root),
        // The test's container may have no memory limit to read.
        gauge: GaugeSource::Given(Box::new(DisabledGauge)),
        job_exe: env!("CARGO_BIN_EXE_cmaeg").into(),
    }
}

#[tokio::test]
async fn setup_runs_the_sandbox_check() {
    let metrics = cmaeg::telemetry::install().unwrap();
    assert!(metrics.render().contains("cmaeg_media_sandbox_ready 0"));
    let bin = bin_dir();
    let root = TempDir::new("media-root", 0o700);
    fs::write(root.path().join("stale"), b"left by an earlier pod").unwrap();
    let runtime = start(setup(bin.path(), root.path())).await.unwrap();
    let scrape = metrics.render();
    assert!(scrape.contains("cmaeg_media_sandbox_ready 1"), "{scrape}");
    assert!(
        scrape.contains(r#"cmaeg_media_transform_total{op="sandbox_check",outcome="ok"} 1"#),
        "{scrape}"
    );
    // Swept at startup, and the self-check's directory removed afterwards.
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    assert_eq!(runtime.root.path(), root.path());
}

#[tokio::test]
async fn setup_refuses_a_group_writable_ffmpeg() {
    let bin = bin_dir();
    fs::set_permissions(bin.path().join("ffmpeg"), fs::Permissions::from_mode(0o775)).unwrap();
    let root = TempDir::new("media-root", 0o700);
    let error = match start(setup(bin.path(), root.path())).await {
        Ok(_) => panic!("a group-writable ffmpeg was accepted"),
        Err(error) => error.to_string(),
    };
    assert_eq!(
        error,
        "media configuration is invalid: CMAEG_MEDIA_FFMPEG_DIR is unusable: \
         ffmpeg must not be group- or other-writable"
    );
}

#[tokio::test]
async fn setup_refuses_an_unlimited_cgroup() {
    let bin = bin_dir();
    let root = TempDir::new("media-root", 0o700);
    let cgroup = TempDir::new("cgroup", 0o700);
    fs::write(cgroup.path().join("memory.max"), "max\n").unwrap();
    let mut unlimited = setup(bin.path(), root.path());
    unlimited.gauge = GaugeSource::Cgroup(cgroup.path().to_path_buf());
    let error = match start(unlimited).await {
        Ok(_) => panic!("an unlimited cgroup was accepted"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.starts_with("media transformer unavailable: the cgroup memory files are unusable"),
        "{error}"
    );
}
