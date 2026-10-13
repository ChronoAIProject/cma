//! The memory watchdog against real jobs.
use super::support::{bin_dir, TempDir};
use bytes::Bytes;
use cmaeg::media::transform::{
    argv::JobOp,
    jobdir::MediaRoot,
    launcher::{JobLauncher, JobResult, JobSpec, ProcessLauncher},
    outcome::TransformOutcome,
    watchdog::{
        CgroupV2Gauge, MemoryGauge, PidfdSignaller, Watchdog, WatchdogHandle, THRESHOLD_RATIO,
    },
};
use std::{
    io,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::time::Instant;

const MIB: u64 = 1024 * 1024;

/// Records the largest sample of the gauge it wraps.
struct Peak<G> {
    inner: G,
    peak: Arc<AtomicU64>,
}

impl<G: MemoryGauge> MemoryGauge for Peak<G> {
    fn limit(&self) -> u64 {
        self.inner.limit()
    }

    fn sample(&mut self, job_pid: u32) -> io::Result<u64> {
        let sample = self.inner.sample(job_pid)?;
        self.peak.fetch_max(sample, Ordering::SeqCst);
        Ok(sample)
    }

    fn inactive_file(&mut self) -> io::Result<u64> {
        self.inner.inactive_file()
    }
}

/// The job's own page tables and resident memory, from its status file.
struct JobGauge {
    limit: u64,
}

impl MemoryGauge for JobGauge {
    fn limit(&self) -> u64 {
        self.limit
    }

    fn sample(&mut self, job_pid: u32) -> io::Result<u64> {
        let status = std::fs::read_to_string(format!("/proc/{job_pid}/status"))?;
        let kib = |name: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(name))
                .and_then(|value| {
                    value
                        .trim()
                        .trim_end_matches("kB")
                        .trim()
                        .parse::<u64>()
                        .ok()
                })
                .unwrap_or(0)
        };
        Ok((kib("VmPTE:") + kib("VmRSS:")) * 1024)
    }

    fn inactive_file(&mut self) -> io::Result<u64> {
        Ok(0)
    }
}

async fn run(watchdog: WatchdogHandle, mode: &str) -> JobResult {
    let bin = bin_dir();
    let root_dir = TempDir::new("media-root", 0o700);
    let dir = MediaRoot::new(root_dir.path()).create_job_dir().unwrap();
    dir.write_input(&Bytes::from(format!("{mode}\n"))).unwrap();
    let launcher = ProcessLauncher::new(
        env!("CARGO_BIN_EXE_cmaeg").into(),
        bin.path().into(),
        Some(watchdog),
    );
    launcher
        .run(JobSpec {
            op: JobOp::Probe,
            job_dir: dir.path().into(),
            deadline: Instant::now() + Duration::from_secs(20),
            stdout_cap: 16_384,
            stdout_headroom: 0,
        })
        .await
}

#[tokio::test]
async fn pte_flood_is_killed_by_watchdog() {
    let peak = Arc::new(AtomicU64::new(0));
    let gauge = Peak {
        inner: JobGauge {
            limit: (64.0 * MIB as f64 / THRESHOLD_RATIO) as u64,
        },
        peak: peak.clone(),
    };
    let watchdog = Watchdog::start(Box::new(gauge), Box::new(PidfdSignaller)).unwrap();
    let threshold = watchdog.threshold();
    let result = run(watchdog, "pte-flood").await;
    assert_eq!(
        result.outcome,
        TransformOutcome::ResourceLimit,
        "{result:?}"
    );
    let peak = peak.load(Ordering::SeqCst);
    assert!(
        peak <= threshold + 64 * MIB,
        "peak {peak}, threshold {threshold}"
    );
}

/// Probe P13, run by hand inside a memory-limited container (the procedure
/// is in #977): the real cgroup gauge kills a page-table flood before the
/// container's OOM killer, and a thread flood does not starve the gateway's
/// own blocking work.
#[test]
#[ignore = "probe P13: needs a container with a memory limit and a pids limit"]
fn watchdog_real_cgroup() {
    let runtime = cmaeg::runtime::build().unwrap();
    let peak = Arc::new(AtomicU64::new(0));
    let gauge = Peak {
        inner: CgroupV2Gauge::open(Path::new("/sys/fs/cgroup")).unwrap(),
        peak: peak.clone(),
    };
    let watchdog = Watchdog::start(Box::new(gauge), Box::new(PidfdSignaller)).unwrap();
    let threshold = watchdog.threshold();
    runtime.block_on(async {
        let flood = run(watchdog, "pte-flood").await;
        assert_eq!(flood.outcome, TransformOutcome::ResourceLimit, "{flood:?}");
    });
    let second = Watchdog::start(
        Box::new(CgroupV2Gauge::open(Path::new("/sys/fs/cgroup")).unwrap()),
        Box::new(PidfdSignaller),
    )
    .unwrap();
    runtime.block_on(async {
        let lookups = futures_util::future::join_all(
            (0..64).map(|_| tokio::spawn(tokio::net::lookup_host("localhost:80"))),
        );
        let (threads, lookups) = tokio::join!(run(second, "thread-flood"), lookups);
        println!("thread_flood_outcome={}", threads.outcome);
        assert!(lookups
            .into_iter()
            .all(|lookup| lookup.is_ok_and(|resolved| resolved.is_ok())));
    });
    println!(
        "watchdog_overshoot_bytes={}",
        peak.load(Ordering::SeqCst).saturating_sub(threshold)
    );
}
