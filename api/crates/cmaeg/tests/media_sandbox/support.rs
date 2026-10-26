//! The sandbox test harness: a bin directory holding the probe under both
//! program names, job directories whose `input` names the probe's mode, and a
//! SIGTERM counter for calls aimed at this test process.
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    os::unix::{
        fs::PermissionsExt,
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Once,
    },
    time::Duration,
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
    job_command(cwd, op, bin, job, arg).output().unwrap()
}

fn job_command(cwd: &Path, op: &str, bin: &Path, job: &Path, arg: Option<&OsStr>) -> Command {
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
    command
}

/// Measure the real wrapper's kernel CPU limit independently of the launcher's
/// equal 20-second wall limit. SCHED_IDLE cannot promise CPU time per wall second.
/// The outer bound is test cleanup only; the wrapper still sets every real limit.
pub async fn observe_cpu_limited_job(bin: &Path, job: &Path) -> (ExitStatus, Duration) {
    let child = job_command(job, "probe", bin, job, None)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut running = ReapOnDrop(Some(child));
    let pid = libc::pid_t::try_from(running.0.as_ref().unwrap().id()).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let mut status = 0;
        // SAFETY: rusage is plain integers; wait4 fills both output values for
        // our exact, unreaped child. No other waiter owns this std::process child.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::wait4(pid, &mut status, libc::WNOHANG, &mut usage) };
        if result == pid {
            // std::process::Child has no kill-on-drop behavior. Disarm cleanup
            // immediately after reaping so it cannot signal a recycled PID.
            drop(running.0.take());
            let time = |value: libc::timeval| {
                Duration::from_secs(u64::try_from(value.tv_sec).unwrap())
                    + Duration::from_micros(u64::try_from(value.tv_usec).unwrap())
            };
            return (
                ExitStatus::from_raw(status),
                time(usage.ru_utime) + time(usage.ru_stime),
            );
        }
        assert!(
            result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR),
            "could not reap sandbox CPU probe: {}",
            std::io::Error::last_os_error()
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "the sandbox CPU probe did not reach its kernel limit before test cleanup"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct ReapOnDrop(Option<Child>);

impl Drop for ReapOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
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
