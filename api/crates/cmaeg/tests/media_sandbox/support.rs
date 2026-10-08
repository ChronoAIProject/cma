//! The sandbox test harness: a bin directory holding the probe under both
//! program names, job directories whose `input` names the probe's mode, and a
//! SIGTERM counter for calls aimed at this test process.
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Once,
    },
};

pub const EPERM: &str = "1";

/// A directory removed with everything in it when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(label: &str, mode: u32) -> Self {
        let path = std::env::temp_dir().join(format!("cmaeg-{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        // Canonical, as the transformer's directory checks require.
        Self(fs::canonicalize(&path).unwrap())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A bin directory (0755) with the probe copied in as `ffmpeg` and `ffprobe`
/// (0755), owned by the test's uid.
pub fn bin_dir() -> TempDir {
    let dir = TempDir::new("media-bin", 0o755);
    for name in ["ffmpeg", "ffprobe"] {
        let path = dir.path().join(name);
        fs::copy(env!("CARGO_BIN_EXE_cmaeg-media-probe"), &path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

/// A job directory (0700) whose `input` (0400) holds `mode`.
pub fn job_dir(mode: &str) -> TempDir {
    let dir = TempDir::new("media-job", 0o700);
    let input = dir.path().join("input");
    fs::write(&input, format!("{mode}\n")).unwrap();
    fs::set_permissions(&input, fs::Permissions::from_mode(0o400)).unwrap();
    dir
}

/// Run `cmaeg --media-job <op> <bin> <job> [arg]` the way the launcher does:
/// no environment, the job directory as cwd and its own process group.
pub fn run_job(op: &str, bin: &Path, job: &Path, arg: Option<&OsStr>) -> Output {
    run_job_in(job, op, bin, job, arg)
}

/// [`run_job`] with `cwd` as the working directory instead of `job`. A `job`
/// argument naming no directory (relative or not UTF-8) must still reach the
/// wrapper: spawning into a missing cwd fails before the wrapper runs.
pub fn run_job_in(cwd: &Path, op: &str, bin: &Path, job: &Path, arg: Option<&OsStr>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cmaeg"));
    command
        .arg("--media-job")
        .arg(op)
        .arg(bin)
        .arg(job)
        .args(arg)
        .env_clear()
        .current_dir(cwd)
        .process_group(0);
    command.output().unwrap()
}

/// Run the probe in `mode` as op `probe`.
pub fn run_mode(mode: &str) -> Output {
    let bin = bin_dir();
    let job = job_dir(mode);
    run_job("probe", bin.path(), job.path(), None)
}

/// The probe's `name=value` lines.
pub fn report(output: &Output) -> BTreeMap<String, String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

static SIGTERMS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn count_sigterm(_signal: libc::c_int) {
    SIGTERMS.fetch_add(1, Ordering::SeqCst);
}

/// Count every SIGTERM this test process receives from now on. nextest runs
/// each test in its own process, so the count is the test's own.
pub fn count_sigterms() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        // SAFETY: the handler only touches an atomic, and `action` is fully
        // initialized before use.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = count_sigterm as *const () as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            assert_eq!(
                libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut()),
                0
            );
        }
    });
}

pub fn sigterms() -> usize {
    SIGTERMS.load(Ordering::SeqCst)
}
