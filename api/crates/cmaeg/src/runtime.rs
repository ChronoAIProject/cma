//! The gateway's Tokio runtime, with a blocking pool that survives thread-creation failure.
use std::{
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Barrier},
    time::Duration,
};
use tokio::runtime::{Builder, Runtime};

/// When the OS refuses a new thread, Tokio queues a blocking task instead of
/// failing only while the pool already counts a thread. With the multi-thread
/// runtime those may be just the runtime's own workers, which never pick the
/// task up, so DNS lookups and file IO stall until a thread can be created
/// again (a current-thread runtime panics instead). A fixed set of blocking
/// threads that never expire turns that failure into a short wait on the set.
/// Sixteen covers DNS, file IO and image decode concurrency without measurable
/// memory; their stacks are part of the gateway's baseline footprint.
const PREWARMED_BLOCKING_THREADS: usize = 16;
/// Tokio's default ten-second keep-alive lets every idle blocking thread exit,
/// which removes the prewarmed threads the queueing fallback relies on. The
/// value stays far below `Duration::MAX` so deadline arithmetic cannot overflow.
const BLOCKING_KEEP_ALIVE: Duration = Duration::from_secs(u32::MAX as u64);
/// Creating sixteen threads takes milliseconds; a pool that is not complete
/// within this bound is starved of threads and must not start serving.
const PREWARM_TIMEOUT: Duration = Duration::from_secs(10);

/// Why the gateway runtime could not be made ready.
#[derive(Debug)]
pub enum RuntimeError {
    /// Tokio refused to build the runtime.
    Build(std::io::Error),
    /// The blocking pool could not start its prewarmed threads.
    Prewarm,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build(error) => write!(formatter, "runtime build failed: {error}"),
            Self::Prewarm => formatter.write_str("blocking pool prewarm failed"),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Build(error) => Some(error),
            Self::Prewarm => None,
        }
    }
}

/// The gateway runtime. Built explicitly so later process modes can run before any runtime exists.
pub fn build() -> Result<Runtime, RuntimeError> {
    let runtime = Builder::new_multi_thread()
        .enable_all()
        .thread_keep_alive(BLOCKING_KEEP_ALIVE)
        .build()
        .map_err(RuntimeError::Build)?;
    if let Err(error) = prewarm(&runtime) {
        // Dropping a runtime waits for its blocking threads, and a failed
        // prewarm can leave some parked on the barrier forever; they end with
        // the process instead.
        runtime.shutdown_background();
        return Err(error);
    }
    Ok(runtime)
}

fn prewarm(runtime: &Runtime) -> Result<(), RuntimeError> {
    start_blocking_threads(runtime, PREWARMED_BLOCKING_THREADS, PREWARM_TIMEOUT)
}

fn start_blocking_threads(
    runtime: &Runtime,
    threads: usize,
    timeout: Duration,
) -> Result<(), RuntimeError> {
    let started = catch_unwind(AssertUnwindSafe(|| {
        runtime.block_on(async {
            // No task returns before all of them are waiting, so the pool
            // cannot reuse a thread and must start one for each task.
            let barrier = Arc::new(Barrier::new(threads));
            let tasks: Vec<_> = (0..threads)
                .map(|_| {
                    let barrier = barrier.clone();
                    tokio::task::spawn_blocking(move || {
                        barrier.wait();
                    })
                })
                .collect();
            tokio::time::timeout(timeout, async {
                for task in tasks {
                    task.await.map_err(|_| RuntimeError::Prewarm)?;
                }
                Ok(())
            })
            .await
            .map_err(|_| RuntimeError::Prewarm)?
        })
    }));
    started.unwrap_or(Err(RuntimeError::Prewarm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::runtime::{Handle, RuntimeFlavor};

    #[test]
    fn builds_the_multi_thread_runtime_with_io_and_time() {
        let runtime = build().unwrap();
        let flavor = runtime.block_on(async {
            // Both drivers are enabled: a timer and a socket need them.
            tokio::time::sleep(Duration::from_millis(1)).await;
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            Handle::current().runtime_flavor()
        });
        assert_eq!(flavor, RuntimeFlavor::MultiThread);
    }

    #[test]
    fn prewarm_fails_when_the_pool_cannot_hold_every_thread() {
        // A pool capped below the prewarm count parks its threads on the
        // barrier and queues the rest, the same shape as a refused thread.
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(3)
            .enable_all()
            .build()
            .unwrap();
        let outcome = start_blocking_threads(&runtime, 4, Duration::from_millis(200));
        assert!(matches!(outcome, Err(RuntimeError::Prewarm)));
        // The parked threads never return, so the runtime must not wait for them.
        runtime.shutdown_background();
    }

    #[test]
    fn prewarm_turns_a_panic_into_an_error() {
        let runtime = Builder::new_multi_thread().enable_all().build().unwrap();
        let outer = Builder::new_current_thread().build().unwrap();
        // Blocking on a runtime from inside another one panics, standing in
        // for Tokio's own panic when no thread can be spawned at all.
        let outcome =
            outer.block_on(async { start_blocking_threads(&runtime, 1, Duration::from_secs(1)) });
        assert!(matches!(outcome, Err(RuntimeError::Prewarm)));
        runtime.shutdown_background();
    }

    #[test]
    fn prewarm_starts_every_thread_within_its_timeout() {
        let runtime = Builder::new_multi_thread().enable_all().build().unwrap();
        start_blocking_threads(&runtime, PREWARMED_BLOCKING_THREADS, PREWARM_TIMEOUT).unwrap();
    }

    #[test]
    fn prewarm_failure_is_reported_without_a_payload() {
        let error = RuntimeError::Prewarm;
        assert_eq!(error.to_string(), "blocking pool prewarm failed");
        assert!(std::error::Error::source(&error).is_none());
        let error = RuntimeError::Build(std::io::Error::other("refused"));
        assert_eq!(error.to_string(), "runtime build failed: refused");
        assert!(std::error::Error::source(&error).is_some());
    }
}
