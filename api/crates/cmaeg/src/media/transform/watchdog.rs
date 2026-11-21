//! The memory watchdog: one dedicated thread that kills the running media job
//! before the container nears its memory limit.
//!
//! `RLIMIT_AS` cannot bound what a hostile job makes the kernel allocate for
//! it: page tables, VMAs and thread stacks are charged to the container's
//! cgroup, about 512 MiB of page tables alone at the default
//! `vm.max_map_count`. With `memory.oom.group` an OOM kills the whole
//! container, gateway included, so the watchdog kills the job first.
//!
//! The thread parks while no job is registered and never allocates while it
//! watches. It signals only through the registered pidfd, and only while it
//! holds the lock with that descriptor present, so it never signals after
//! [`Registration::finish`] or a reused descriptor.
use std::{
    io,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Page-table creation, the fastest charge path left, is serialized on the
/// job's `mmap_lock` and adds tens of MiB per 10 ms: far below the 10 % of
/// headroom (about 205 MiB at 2Gi) between the threshold and the limit.
pub const PERIOD: Duration = Duration::from_millis(10);
/// The benign memory gate keeps the container at or below 0.8 × its limit, so
/// the watchdog fires only on a gate violation or a hostile job; the job runs
/// at `SCHED_IDLE` and the watchdog at normal priority.
pub const THRESHOLD_RATIO: f64 = 0.9;

/// The container's memory, as the watchdog sees it.
pub trait MemoryGauge: Send + 'static {
    /// The container's memory limit, in bytes.
    fn limit(&self) -> u64;
    /// Current usage in bytes. `job_pid` exists for test gauges that measure
    /// one process; the cgroup gauge measures the whole container.
    fn sample(&mut self, job_pid: u32) -> io::Result<u64>;
    /// Reclaimable page cache, which kubelet subtracts for the working set.
    fn inactive_file(&mut self) -> io::Result<u64>;
}

/// How the watchdog kills a job.
pub trait Signaller: Send + 'static {
    fn kill(&mut self, pidfd: BorrowedFd<'_>) -> io::Result<()>;
}

/// A gauge that never fires: build containers have no memory limit, so the
/// build-time self-test runs without one.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisabledGauge;

impl MemoryGauge for DisabledGauge {
    fn limit(&self) -> u64 {
        u64::MAX
    }

    fn sample(&mut self, _job_pid: u32) -> io::Result<u64> {
        Ok(0)
    }

    fn inactive_file(&mut self) -> io::Result<u64> {
        Ok(0)
    }
}

/// The threshold for `limit`: `THRESHOLD_RATIO` of it.
pub fn threshold(limit: u64) -> u64 {
    (limit as f64 * THRESHOLD_RATIO) as u64
}

struct Watched {
    pid: u32,
    pidfd: OwnedFd,
    killed: bool,
}

#[derive(Default)]
struct Shared {
    watched: Mutex<Option<Watched>>,
    stopped: AtomicBool,
    /// Samples that could not be read; the loop cannot log without
    /// allocating, so callers report them.
    sample_errors: AtomicU64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Option<Watched>> {
        // A poisoned lock only means a panic elsewhere; the slot is still
        // consistent, because every critical section is a single assignment.
        self.watched
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Starts the watchdog thread.
pub struct Watchdog;

impl Watchdog {
    /// Start the `cmaeg-media-watchdog` thread. Starting it at startup means
    /// thread exhaustion later can never stop it.
    pub fn start(
        gauge: Box<dyn MemoryGauge>,
        signaller: Box<dyn Signaller>,
    ) -> io::Result<WatchdogHandle> {
        let shared = Arc::new(Shared::default());
        let limit = gauge.limit();
        let threshold = threshold(limit);
        let state = shared.clone();
        let thread = thread::Builder::new()
            .name("cmaeg-media-watchdog".into())
            .spawn(move || watch(&state, gauge, signaller, threshold))?;
        Ok(WatchdogHandle {
            shared,
            thread: Some(thread),
            limit,
            threshold,
        })
    }
}

fn watch(
    shared: &Shared,
    mut gauge: Box<dyn MemoryGauge>,
    mut signaller: Box<dyn Signaller>,
    threshold: u64,
) {
    while !shared.stopped.load(Ordering::Acquire) {
        // Copied out in its own statement so the guard is released before
        // the thread parks or sleeps: a guard held across `park` would block
        // the `register` that is meant to wake it.
        let state = shared
            .lock()
            .as_ref()
            .map(|watched| (watched.pid, watched.killed));
        let pid = match state {
            Some((pid, false)) => pid,
            Some((_, true)) => {
                thread::sleep(PERIOD);
                continue;
            }
            None => {
                thread::park();
                continue;
            }
        };
        if over(&mut *gauge, &shared.sample_errors, pid, threshold) {
            let mut watched = shared.lock();
            if let Some(watched) = watched.as_mut().filter(|watched| watched.pid == pid) {
                if signaller.kill(watched.pidfd.as_fd()).is_err() {
                    shared.sample_errors.fetch_add(1, Ordering::Relaxed);
                }
                watched.killed = true;
            }
        }
        thread::sleep(PERIOD);
    }
}

/// Whether usage, less reclaimable page cache, has reached `threshold`. A
/// sample that cannot be read is counted and skipped; an unreadable
/// `inactive_file` counts as zero, erring towards the kill.
fn over(gauge: &mut dyn MemoryGauge, errors: &AtomicU64, pid: u32, threshold: u64) -> bool {
    let sample = match gauge.sample(pid) {
        Ok(sample) => sample,
        Err(_) => {
            errors.fetch_add(1, Ordering::Relaxed);
            return false;
        }
    };
    if sample < threshold {
        return false;
    }
    let inactive = gauge.inactive_file().unwrap_or(0);
    sample.saturating_sub(inactive) >= threshold
}

/// The running watchdog. Dropping it stops the thread.
pub struct WatchdogHandle {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    limit: u64,
    threshold: u64,
}

impl WatchdogHandle {
    /// Watch the job `pid` through `pidfd` until the registration finishes.
    /// One job runs at a time, so a new registration replaces any other.
    pub fn register(&self, pid: u32, pidfd: OwnedFd) -> Registration {
        *self.shared.lock() = Some(Watched {
            pid,
            pidfd,
            killed: false,
        });
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
        Registration {
            shared: self.shared.clone(),
            pid,
            finished: false,
        }
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn threshold(&self) -> u64 {
        self.threshold
    }

    /// Samples the gauge failed to read, and kills that failed, so far.
    pub fn sample_errors(&self) -> u64 {
        self.shared.sample_errors.load(Ordering::Relaxed)
    }
}

impl Drop for WatchdogHandle {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            if thread.join().is_err() {
                tracing::error!("media watchdog thread panicked");
            }
        }
    }
}

/// One watched job. [`Registration::finish`] takes the descriptor back and
/// reports whether the watchdog killed the job; dropping it does the same.
pub struct Registration {
    shared: Arc<Shared>,
    pid: u32,
    finished: bool,
}

impl Registration {
    pub fn finish(mut self) -> bool {
        self.finished = true;
        self.take()
    }

    fn take(&self) -> bool {
        let mut watched = self.shared.lock();
        match watched.take() {
            Some(entry) if entry.pid == self.pid => entry.killed,
            // Another job's registration: leave it in place.
            other => {
                *watched = other;
                false
            }
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if !self.finished {
            self.take();
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use linux::{CgroupV2Gauge, PidfdSignaller};

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux;

#[cfg(test)]
mod tests;
