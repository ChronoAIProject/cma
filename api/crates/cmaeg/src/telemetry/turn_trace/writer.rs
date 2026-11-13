//! Two bounded stages keep serialization and stdout off delivery tasks.
use super::record::Record;
use std::{
    io::{self, Write},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
#[cfg(test)]
use tracing_appender::non_blocking::ErrorCounter;
use tracing_appender::non_blocking::{NonBlocking, NonBlockingBuilder, WorkerGuard};
use uuid::Uuid;

// At most 4096 queued records across both stages. Typed records contain no
// heap allocations; each serialized trace line is limited to 2 KiB.
const RECORD_CAPACITY: usize = 2048;
const LINE_CAPACITY: usize = 2048;
const HEALTH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub(crate) struct Writer {
    sender: SyncSender<Record>,
    dropped: Arc<AtomicU64>,
    #[cfg(test)]
    output_dropped: ErrorCounter,
}

impl Writer {
    pub(super) fn send(&self, record: Record) {
        if self.sender.try_send(record).is_err() {
            // No synchronous logger, formatting or output lock on overflow.
            let _ = self
                .dropped
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    Some(n.saturating_add(1))
                });
        }
    }

    #[cfg(test)]
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
            .load(Ordering::Relaxed)
            .saturating_add(self.output_dropped.dropped_lines() as u64)
    }
}

pub(super) struct Output {
    pub writer: Writer,
    pub logs: BoundedLogs,
    pub guard: WorkerGuard,
}

/// Ordinary events share the sink, so they obey the same queued-byte bound.
#[derive(Clone)]
pub(crate) struct BoundedLogs {
    output: NonBlocking,
    dropped: Arc<AtomicU64>,
}

impl Write for BoundedLogs {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > super::record::MAX_RECORD_BYTES {
            let _ = self
                .dropped
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    Some(n.saturating_add(1))
                });
            return Ok(bytes.len());
        }
        self.output.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

/// Ordinary logs share the nonblocking sink when tracing is enabled. The sink
/// receives an unlocked stdout handle and acquires its lock only while writing.
pub(super) fn start<W: Write + Send + 'static>(output: W) -> io::Result<Output> {
    let dropped = Arc::new(AtomicU64::new(0));
    let output = CountedOutput {
        output,
        dropped: Arc::clone(&dropped),
    };
    // The library panics if its worker cannot spawn. Diagnostics must remain
    // optional even under thread/resource exhaustion.
    let (logs, guard) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        NonBlockingBuilder::default()
            .buffered_lines_limit(LINE_CAPACITY)
            .lossy(true)
            .thread_name("cmaeg-log-output")
            .finish(output)
    }))
    .map_err(|_| io::Error::other("diagnostic output worker unavailable"))?;
    let writer = spawn_serializer(logs.clone(), RECORD_CAPACITY, dropped.clone())?;
    Ok(Output {
        writer,
        logs: BoundedLogs {
            output: logs,
            dropped,
        },
        guard,
    })
}

fn spawn_serializer(
    mut output: NonBlocking,
    capacity: usize,
    dropped: Arc<AtomicU64>,
) -> io::Result<Writer> {
    let (sender, receiver) = mpsc::sync_channel(capacity);
    #[cfg(test)]
    let output_dropped = output.error_counter();
    let worker_dropped = Arc::clone(&dropped);
    thread::Builder::new()
        .name("cmaeg-turn-trace".into())
        .spawn(move || serialize(receiver, &mut output, worker_dropped))?;
    Ok(Writer {
        sender,
        dropped,
        #[cfg(test)]
        output_dropped,
    })
}

/// The appender does not expose sink IO failures. Count them here without
/// passing an arbitrary IO error to its shutdown stderr diagnostics.
struct CountedOutput<W> {
    output: W,
    dropped: Arc<AtomicU64>,
}

impl<W: Write> Write for CountedOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.output.write_all(bytes).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.output.flush().is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

fn serialize(receiver: Receiver<Record>, output: &mut NonBlocking, dropped: Arc<AtomicU64>) {
    let instance_id = Uuid::new_v4();
    let output_dropped = output.error_counter();
    let mut reported = 0;
    let mut last_health = Instant::now();
    loop {
        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(record) => {
                // One write queues one complete NDJSON line, avoiding interleaved
                // fragments from simultaneous ordinary service log messages.
                if record
                    .encode(instance_id)
                    .is_none_or(|bytes| output.write_all(&bytes).is_err())
                {
                    let _ = dropped.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                        Some(n.saturating_add(1))
                    });
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let total = dropped
            .load(Ordering::Relaxed)
            .saturating_add(output_dropped.dropped_lines() as u64);
        if total > reported && last_health.elapsed() >= HEALTH_INTERVAL {
            metrics::counter!("cmaeg_turn_trace_dropped_total").absolute(total);
            // A fixed health message uses the same lossy output queue. Never
            // call the synchronous logger, even from this background worker.
            let warning = format!(
                "{{\"level\":\"WARN\",\"target\":\"cmaeg::turn_trace\",\"message\":\"diagnostic records dropped\",\"dropped_total\":{total}}}\n"
            );
            if output.write_all(warning.as_bytes()).is_err() {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
            reported = total;
            last_health = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests;
