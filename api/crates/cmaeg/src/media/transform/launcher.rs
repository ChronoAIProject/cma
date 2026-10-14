//! Running one media job: the [`JobLauncher`] seam, and on Linux x86_64 the
//! [`ProcessLauncher`] that executes the sandbox wrapper.
//!
//! A job's result is accepted only after the process is reaped. Its stdout is
//! read into a preallocated buffer up to a cap while its stderr is drained,
//! so a chatty job can neither block until its deadline nor die of SIGPIPE.
use super::{
    argv::JobOp,
    outcome::{StderrClass, TransformOutcome},
};
use crate::telemetry::names;
use bytes::BytesMut;
use futures_util::future::BoxFuture;
use std::{fmt, path::PathBuf, time::Duration};
use tokio::time::Instant;

/// Every process gets at most this long, whatever its set's deadline.
pub const JOB_WALL: Duration = Duration::from_secs(20);
/// The startup self-check's deadline.
pub const SANDBOX_CHECK_WALL: Duration = Duration::from_secs(5);

/// One job to run.
#[derive(Debug, Clone)]
pub struct JobSpec {
    pub op: JobOp,
    /// The job's working directory, holding its `input`.
    pub job_dir: PathBuf,
    /// Capped at [`JOB_WALL`] from the start.
    pub deadline: Instant,
    /// Most stdout bytes kept.
    pub stdout_cap: usize,
    /// Zero bytes reserved in front of the output (a WAV header's 44).
    pub stdout_headroom: usize,
}

impl JobSpec {
    /// The startup self-check: `job_dir` is only its working directory, and
    /// any stdout at all is `output_invalid`.
    pub fn sandbox_check(job_dir: PathBuf) -> Self {
        Self {
            op: JobOp::SandboxCheck,
            job_dir,
            deadline: Instant::now() + SANDBOX_CHECK_WALL,
            stdout_cap: 0,
            stdout_headroom: 0,
        }
    }

    /// The deadline the process actually runs under.
    pub fn effective_deadline(&self, started: Instant) -> Instant {
        self.deadline.min(started + JOB_WALL)
    }
}

/// What one job produced.
pub struct JobResult {
    pub outcome: TransformOutcome,
    /// `stdout_headroom` zero bytes, then at most `stdout_cap` output bytes.
    pub stdout: BytesMut,
    pub elapsed: Duration,
    pub cpu: Duration,
    pub stderr_class: StderrClass,
}

/// Output can quote input, so only its size is shown.
impl fmt::Debug for JobResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JobResult")
            .field("outcome", &self.outcome)
            .field("stdout_bytes", &self.stdout.len())
            .field("elapsed", &self.elapsed)
            .field("cpu", &self.cpu)
            .field("stderr_class", &self.stderr_class)
            .finish()
    }
}

impl JobResult {
    /// A job that never ran.
    pub fn unavailable(outcome: TransformOutcome) -> Self {
        Self {
            outcome,
            stdout: BytesMut::new(),
            elapsed: Duration::ZERO,
            cpu: Duration::ZERO,
            stderr_class: StderrClass::None,
        }
    }
}

/// Runs one job to completion and reports it; never fails, only classifies.
pub trait JobLauncher: Send + Sync + 'static {
    fn run(&self, spec: JobSpec) -> BoxFuture<'_, JobResult>;
}

/// Composition holds the platform's launcher without naming its type, which
/// exists on Linux x86_64 only.
impl JobLauncher for Box<dyn JobLauncher> {
    fn run(&self, spec: JobSpec) -> BoxFuture<'_, JobResult> {
        (**self).run(spec)
    }
}

/// Count and time one finished job under its final outcome, inside the
/// `cmaeg.media.transform` span. Nothing the job wrote is logged.
pub fn observe(op: &JobOp, result: &JobResult) {
    let outcome = result.outcome.as_str();
    metrics::counter!(
        names::MEDIA_TRANSFORM_TOTAL,
        names::OP => op.as_str(),
        names::OUTCOME => outcome
    )
    .increment(1);
    metrics::histogram!(names::MEDIA_TRANSFORM_SECONDS, names::OP => op.as_str())
        .record(result.elapsed.as_secs_f64());
    let span = tracing::info_span!(
        "cmaeg.media.transform",
        op = op.as_str(),
        outcome,
        ms = result.elapsed.as_millis() as u64,
        cpu_ms = result.cpu.as_millis() as u64,
        stdout_bytes = result.stdout.len(),
    );
    span.in_scope(|| {
        if result.outcome == TransformOutcome::Ok {
            tracing::debug!("media job finished");
        } else {
            tracing::info!("media job failed");
        }
    });
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use process::ProcessLauncher;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod process {
    use super::{JobLauncher, JobResult, JobSpec};
    use crate::media::transform::{
        argv,
        outcome::{classify, ClassifyInput, Exit, StderrClass, TransformOutcome, STDERR_HEAD},
        watchdog::{Registration, WatchdogHandle},
    };
    use bytes::BytesMut;
    use futures_util::future::BoxFuture;
    use std::{
        os::{
            fd::{FromRawFd, OwnedFd},
            unix::process::ExitStatusExt,
        },
        path::PathBuf,
        process::{ExitStatus, Stdio},
        sync::Arc,
        time::Duration,
    };
    use tokio::{
        io::AsyncReadExt,
        process::{Child, ChildStderr, ChildStdout, Command},
        sync::{OwnedSemaphorePermit, Semaphore},
        time::{sleep_until, timeout_at, Instant},
    };

    const CHUNK: usize = 16 * 1024;

    /// Executes `job_exe --media-job ...`, one process at a time, each
    /// watched by the memory watchdog when there is one.
    pub struct ProcessLauncher {
        job_exe: PathBuf,
        bin_dir: PathBuf,
        watchdog: Option<WatchdogHandle>,
        /// Held from before a spawn until that process is reaped, even when
        /// the caller is cancelled, so processes never overlap and the
        /// `RUSAGE_CHILDREN` difference is exactly one job's.
        reaped: Arc<Semaphore>,
    }

    impl ProcessLauncher {
        /// `job_exe` is `current_exe()` in production and the test build's
        /// `cmaeg` binary in tests.
        pub fn new(job_exe: PathBuf, bin_dir: PathBuf, watchdog: Option<WatchdogHandle>) -> Self {
            Self {
                job_exe,
                bin_dir,
                watchdog,
                reaped: Arc::new(Semaphore::new(1)),
            }
        }

        /// The watchdog's registration for job `pid`, holding a duplicate of
        /// its pidfd; `Some(None)` without a watchdog, `None` when the
        /// descriptor cannot be duplicated.
        fn watch(&self, pid: u32, pidfd: &OwnedFd) -> Option<Option<Registration>> {
            let Some(watchdog) = &self.watchdog else {
                return Some(None);
            };
            let duplicate = pidfd.try_clone().ok()?;
            Some(Some(watchdog.register(pid, duplicate)))
        }
    }

    impl JobLauncher for ProcessLauncher {
        fn run(&self, spec: JobSpec) -> BoxFuture<'_, JobResult> {
            Box::pin(self.run_job(spec))
        }
    }

    /// The child until it is reaped. Dropped earlier (the caller was
    /// cancelled), it kills and reaps the child in a task that releases the
    /// process slot only afterwards.
    struct Running {
        child: Option<Child>,
        slot: Option<OwnedSemaphorePermit>,
    }

    impl Running {
        fn child(&mut self) -> &mut Child {
            self.child
                .as_mut()
                .expect("the child is held until it is reaped")
        }

        fn reaped(mut self) -> Option<OwnedSemaphorePermit> {
            self.child = None;
            self.slot.take()
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            let Some(mut child) = self.child.take() else {
                return;
            };
            let slot = self.slot.take();
            let _ = child.start_kill();
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => {
                    handle.spawn(async move {
                        if let Err(error) = child.wait().await {
                            tracing::warn!(kind = ?error.kind(), "cancelled media job not reaped");
                        }
                        drop(slot);
                    });
                }
                // Without a runtime, kill_on_drop and tokio's orphan reaper
                // finish the child.
                Err(_) => tracing::warn!("cancelled media job left to the orphan reaper"),
            }
        }
    }

    /// Bytes read from the job and how reading ended.
    struct Output {
        stdout: BytesMut,
        stderr: Vec<u8>,
        overflowed: bool,
    }

    impl ProcessLauncher {
        async fn run_job(&self, spec: JobSpec) -> JobResult {
            let started = Instant::now();
            let deadline = spec.effective_deadline(started);
            let Ok(Ok(slot)) = timeout_at(deadline, self.reaped.clone().acquire_owned()).await
            else {
                return JobResult::unavailable(TransformOutcome::Timeout);
            };
            let cpu_before = children_cpu();
            let spawned_at = Instant::now();
            let spawned = Command::new(&self.job_exe)
                .args(argv::launch_args(&spec.op, &self.bin_dir, &spec.job_dir))
                .env_clear()
                .current_dir(&spec.job_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .process_group(0)
                .spawn();
            let mut running = match spawned {
                Ok(child) => Running {
                    child: Some(child),
                    slot: Some(slot),
                },
                Err(error) => {
                    tracing::warn!(kind = ?error.kind(), op = spec.op.as_str(), "media job could not be spawned");
                    return JobResult::unavailable(TransformOutcome::SandboxUnavailable);
                }
            };
            // Nothing has reaped the child yet, so its pid still names it.
            let watched = running
                .child()
                .id()
                .and_then(|pid| Some((pid, pidfd_open(pid)?)))
                .and_then(|(pid, pidfd)| self.watch(pid, &pidfd).map(|watch| (pidfd, watch)));
            let Some((_pidfd, registration)) = watched else {
                let _ = running.child().start_kill();
                let _ = running.child().wait().await;
                drop(running.reaped());
                return JobResult::unavailable(TransformOutcome::SandboxUnavailable);
            };
            let errors_before = self.watchdog.as_ref().map(WatchdogHandle::sample_errors);
            let (Some(stdout), Some(stderr)) =
                (running.child().stdout.take(), running.child().stderr.take())
            else {
                return JobResult::unavailable(TransformOutcome::SandboxUnavailable);
            };
            let truncate = matches!(spec.op, argv::JobOp::Audio(_));
            let mut deadline_killed = false;
            let (output, status) = drive(
                &mut running,
                Pipes { stdout, stderr },
                &spec,
                truncate,
                deadline,
                &mut deadline_killed,
            )
            .await;
            let elapsed = spawned_at.elapsed();
            let cpu = children_cpu().saturating_sub(cpu_before);
            drop(running.reaped());
            let watchdog_killed = registration.is_some_and(Registration::finish);
            let errors = self.watchdog.as_ref().map(WatchdogHandle::sample_errors);
            if errors > errors_before {
                tracing::warn!(
                    op = spec.op.as_str(),
                    "the memory watchdog could not read or signal during a media job"
                );
            }
            let exit = match status {
                Some(status) => exit_of(status),
                None => return JobResult::unavailable(TransformOutcome::SandboxUnavailable),
            };
            let outcome = classify(&ClassifyInput {
                deadline_killed,
                watchdog_killed,
                overflow_killed: output.overflowed,
                exit,
                cpu,
                output_valid: true,
            });
            JobResult {
                outcome,
                stdout: output.stdout,
                elapsed,
                cpu,
                stderr_class: StderrClass::of(&output.stderr),
            }
        }
    }

    struct Pipes {
        stdout: ChildStdout,
        stderr: ChildStderr,
    }

    /// Read both pipes to EOF and reap the child, killing it at `deadline`
    /// or, for an op whose output may not be truncated, past its cap.
    async fn drive(
        running: &mut Running,
        mut pipes: Pipes,
        spec: &JobSpec,
        truncate: bool,
        deadline: Instant,
        deadline_killed: &mut bool,
    ) -> (Output, Option<ExitStatus>) {
        let mut output = Output {
            stdout: BytesMut::with_capacity(spec.stdout_headroom + spec.stdout_cap),
            stderr: Vec::with_capacity(STDERR_HEAD),
            overflowed: false,
        };
        output.stdout.resize(spec.stdout_headroom, 0);
        let limit = spec.stdout_headroom + spec.stdout_cap;
        let (mut out_chunk, mut err_chunk) = ([0_u8; CHUNK], [0_u8; CHUNK]);
        let (mut out_open, mut err_open) = (true, true);
        let mut status = None;
        let timer = sleep_until(deadline);
        tokio::pin!(timer);
        while out_open || err_open || status.is_none() {
            tokio::select! {
                read = pipes.stdout.read(&mut out_chunk), if out_open => match read {
                    Ok(0) | Err(_) => out_open = false,
                    Ok(count) => {
                        let room = limit - output.stdout.len();
                        output.stdout.extend_from_slice(&out_chunk[..count.min(room)]);
                        if count > room && !truncate && !output.overflowed {
                            output.overflowed = true;
                            let _ = running.child().start_kill();
                        }
                    }
                },
                read = pipes.stderr.read(&mut err_chunk), if err_open => match read {
                    Ok(0) | Err(_) => err_open = false,
                    Ok(count) => {
                        let room = STDERR_HEAD - output.stderr.len();
                        output.stderr.extend_from_slice(&err_chunk[..count.min(room)]);
                    }
                },
                exit = running.child().wait(), if status.is_none() => match exit {
                    Ok(exit) => status = Some(exit),
                    Err(error) => {
                        tracing::warn!(kind = ?error.kind(), "media job could not be awaited");
                        return (output, None);
                    }
                },
                () = &mut timer, if !*deadline_killed => {
                    *deadline_killed = true;
                    let _ = running.child().start_kill();
                }
            }
        }
        (output, status)
    }

    fn exit_of(status: ExitStatus) -> Exit {
        match (status.code(), status.signal()) {
            (Some(code), _) => Exit::Code(code),
            (None, Some(signal)) => Exit::Signal(signal),
            (None, None) => Exit::Code(-1),
        }
    }

    /// A pidfd for `pid`, through the raw syscall musl may lack a wrapper for.
    fn pidfd_open(pid: u32) -> Option<OwnedFd> {
        let pid = libc::pid_t::try_from(pid).ok()?;
        // SAFETY: pidfd_open takes integer arguments and returns a new fd.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        let fd = i32::try_from(fd).ok().filter(|fd| *fd >= 0)?;
        // SAFETY: the kernel just returned `fd` to this process.
        Some(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// User and system CPU of every reaped child.
    fn children_cpu() -> Duration {
        // SAFETY: rusage is plain integers, and getrusage fills it.
        let usage = unsafe {
            let mut usage: libc::rusage = std::mem::zeroed();
            libc::getrusage(libc::RUSAGE_CHILDREN, &mut usage);
            usage
        };
        let time = |value: libc::timeval| {
            Duration::from_secs(u64::try_from(value.tv_sec).unwrap_or(0))
                + Duration::from_micros(u64::try_from(value.tv_usec).unwrap_or(0))
        };
        time(usage.ru_utime) + time(usage.ru_stime)
    }
}
