//! The media wrapper and launcher end to end: deadlines, the CPU limit, stdout caps,
//! stderr draining, exit classification and cancellation.
use super::support::{bin_dir, observe_cpu_limited_job, TempDir};
use bytes::Bytes;
use cmaeg::media::transform::{
    argv::{AudioSeconds, FrameTime, JobOp},
    jobdir::{JobDir, MediaRoot},
    launcher::{JobLauncher, JobResult, JobSpec, ProcessLauncher},
    outcome::{classify, ClassifyInput, Exit, StderrClass, TransformOutcome},
};
use std::{os::unix::process::ExitStatusExt, time::Duration};
use tokio::time::Instant;

struct Fixture {
    bin: TempDir,
    root_dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            bin: bin_dir(),
            root_dir: TempDir::new("media-root", 0o700),
        }
    }

    fn root(&self) -> MediaRoot {
        MediaRoot::new(self.root_dir.path())
    }

    fn launcher(&self) -> ProcessLauncher {
        ProcessLauncher::new(
            env!("CARGO_BIN_EXE_cmaeg").into(),
            self.bin.path().into(),
            None,
        )
    }

    fn job(&self, mode: &str) -> JobDir {
        let dir = self.root().create_job_dir().unwrap();
        dir.write_input(&Bytes::from(format!("{mode}\n"))).unwrap();
        dir
    }
}

fn spec(op: JobOp, dir: &JobDir, within: Duration, cap: usize, headroom: usize) -> JobSpec {
    JobSpec {
        op,
        job_dir: dir.path().into(),
        deadline: Instant::now() + within,
        stdout_cap: cap,
        stdout_headroom: headroom,
    }
}

async fn run(fixture: &Fixture, mode: &str, op: JobOp, within: Duration, cap: usize) -> JobResult {
    let dir = fixture.job(mode);
    fixture.launcher().run(spec(op, &dir, within, cap, 0)).await
}

/// Every live process whose process group is `pgid`.
fn group_members(pgid: i32) -> Vec<i32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| {
                    let (_, rest) = stat.rsplit_once(')')?;
                    rest.split_whitespace().nth(2)?.parse::<i32>().ok()
                })
                == Some(pgid)
        })
        .collect()
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

#[tokio::test]
async fn sleep_is_killed_at_deadline() {
    let fixture = Fixture::new();
    let started = Instant::now();
    let result = run(
        &fixture,
        "sleep",
        JobOp::Probe,
        Duration::from_secs(1),
        16_384,
    )
    .await;
    assert_eq!(result.outcome, TransformOutcome::Timeout);
    assert!(started.elapsed() < Duration::from_secs(5));
    let text = String::from_utf8_lossy(&result.stdout).into_owned();
    let pid: i32 = text
        .lines()
        .find_map(|line| line.strip_prefix("pid="))
        .and_then(|pid| pid.parse().ok())
        .unwrap_or_else(|| panic!("no pid in {text:?}"));
    // SAFETY: signal 0 only checks for existence.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(errno(), libc::ESRCH);
    assert!(group_members(pid).is_empty());
}

/// Test the raw wrapper's CPU enforcement without racing the launcher's equal
/// wall limit. SCHED_IDLE may receive less than one CPU even with four threads.
#[tokio::test]
async fn raw_wrapper_spin_is_killed_at_its_cpu_limit() {
    let fixture = Fixture::new();
    let dir = fixture.job("spin");
    let (status, cpu) = observe_cpu_limited_job(fixture.bin.path(), dir.path()).await;
    assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
    assert!(
        cpu >= Duration::from_millis(19_500) && cpu < Duration::from_secs(25),
        "{cpu:?}"
    );
    assert_eq!(
        classify(&ClassifyInput {
            deadline_killed: false,
            watchdog_killed: false,
            overflow_killed: false,
            exit: Exit::Signal(status.signal().unwrap()),
            cpu,
            output_valid: true,
        }),
        TransformOutcome::CpuLimit
    );
}

#[tokio::test]
async fn frame_stdout_over_cap_is_output_invalid() {
    let fixture = Fixture::new();
    let started = Instant::now();
    let frame = JobOp::Frame(FrameTime::parse("1.000").unwrap());
    let result = run(
        &fixture,
        "stdout-flood",
        frame,
        Duration::from_secs(20),
        262_144,
    )
    .await;
    assert_eq!(result.outcome, TransformOutcome::OutputInvalid);
    assert_eq!(result.stdout.len(), 262_144);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn audio_stdout_over_cap_is_truncated() {
    let fixture = Fixture::new();
    let dir = fixture.job("stdout-flood 100000");
    let audio = JobOp::Audio(AudioSeconds::new(1).unwrap());
    let result = fixture
        .launcher()
        .run(spec(audio, &dir, Duration::from_secs(20), 32_000, 44))
        .await;
    assert_eq!(result.outcome, TransformOutcome::Ok, "{result:?}");
    assert_eq!(result.stdout.len(), 44 + 32_000);
    assert!(result.stdout[..44].iter().all(|byte| *byte == 0));
    assert!(result.stdout[44..].iter().all(|byte| *byte == b'o'));
}

#[tokio::test]
async fn stderr_flood_does_not_block() {
    let fixture = Fixture::new();
    let started = Instant::now();
    let result = run(
        &fixture,
        "stderr-flood",
        JobOp::Probe,
        Duration::from_secs(20),
        16_384,
    )
    .await;
    assert_eq!(result.outcome, TransformOutcome::Ok);
    assert_eq!(result.stderr_class, StderrClass::Other);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn exit_125_is_sandbox_unavailable() {
    let fixture = Fixture::new();
    let result = run(
        &fixture,
        "exit 125",
        JobOp::Probe,
        Duration::from_secs(20),
        16_384,
    )
    .await;
    assert_eq!(result.outcome, TransformOutcome::SandboxUnavailable);
}

#[tokio::test]
async fn exit_1_is_exit_nonzero() {
    let fixture = Fixture::new();
    let result = run(
        &fixture,
        "exit 1",
        JobOp::Probe,
        Duration::from_secs(20),
        16_384,
    )
    .await;
    assert_eq!(result.outcome, TransformOutcome::ExitNonzero);
}

#[tokio::test]
async fn sandbox_check_runs_through_the_launcher() {
    let fixture = Fixture::new();
    let dir = fixture.root().create_job_dir().unwrap();
    let result = fixture
        .launcher()
        .run(JobSpec::sandbox_check(dir.path().into()))
        .await;
    assert_eq!(result.outcome, TransformOutcome::Ok, "{result:?}");
    assert!(result.stdout.is_empty());
}

/// The probe running as a child of this process, by its program name.
fn running_probe() -> Option<i32> {
    let me = std::process::id().to_string();
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .find(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                stat.contains("(ffprobe)")
                    && stat
                        .rsplit_once(')')
                        .and_then(|(_, rest)| rest.split_whitespace().nth(1))
                        == Some(me.as_str())
            })
        })
}

#[tokio::test]
async fn cancelled_job_is_reaped_before_the_slot_frees() {
    let fixture = Fixture::new();
    let launcher = fixture.launcher();
    let sleeping = fixture.job("sleep");
    // A boxed future: dropping it is the cancellation.
    let mut job = launcher.run(spec(
        JobOp::Probe,
        &sleeping,
        Duration::from_secs(20),
        16_384,
        0,
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    let pid = loop {
        tokio::select! {
            _ = &mut job => panic!("the sleeping job finished"),
            () = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
        if let Some(pid) = running_probe() {
            break pid;
        }
        assert!(Instant::now() < deadline, "the sleeping job never started");
    };
    drop(job);
    // The next job can only start once the cancelled one is reaped, so its
    // /proc entry (a zombie until then) is gone when the next job returns.
    let next = fixture.job("exit 0");
    let result = launcher
        .run(spec(
            JobOp::Probe,
            &next,
            Duration::from_secs(20),
            16_384,
            0,
        ))
        .await;
    assert_eq!(result.outcome, TransformOutcome::Ok);
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}
