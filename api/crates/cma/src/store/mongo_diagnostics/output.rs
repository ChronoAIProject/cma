//! Bounded metadata transport: callbacks never format records or write to a sink.
use super::{summary, Command};
use std::{
    io,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
const CAPACITY: usize = 4096;
const INTERVAL: Duration = Duration::from_secs(10);

pub(in crate::store) enum Record {
    Detail {
        command: Command,
        event: &'static str,
        duration: Duration,
        docs: Option<u64>,
        hold_us: u64,
        deferred: bool,
    },
    Summary(summary::Row),
    Latency {
        pool: &'static str,
        operation: &'static str,
        window_ms: u64,
        count: u64,
        failures: u64,
        total_us: u64,
        max_us: u64,
        buckets: [u64; 7],
        in_use: usize,
        in_use_peak: usize,
        untracked: u64,
    },
    Circuit {
        operation: &'static str,
        reason: &'static str,
        elapsed_ms: u64,
        failed: bool,
    },
    Pool {
        pool: &'static str,
        commands: usize,
        holds: usize,
        oldest_hold_us: u64,
        untracked: u64,
    },
    #[cfg(test)]
    Barrier(mpsc::Sender<()>),
}
struct Envelope {
    record: Record,
    dispatch: tracing::Dispatch,
    queued_at: Instant,
}
struct Queue<T> {
    sender: SyncSender<T>,
    dropped: Arc<AtomicU64>,
}
impl<T: Send + 'static> Queue<T> {
    fn spawn(
        capacity: usize,
        interval: Duration,
        mut write: impl FnMut(T) + Send + 'static,
        mut report: impl FnMut(u64) + Send + 'static,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let losses = dropped.clone();
        let _worker = std::thread::Builder::new()
            .name("mongo-diagnostic-writer".into())
            .spawn(move || {
                drain(receiver, &losses, interval, &mut write, &mut report);
            })?;
        Ok(Self { sender, dropped })
    }
    fn enqueue(&self, record: T) {
        if self.sender.try_send(record).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
fn drain<T>(
    receiver: Receiver<T>,
    dropped: &AtomicU64,
    interval: Duration,
    write: &mut impl FnMut(T),
    report: &mut impl FnMut(u64),
) {
    let mut next = Instant::now() + interval;
    loop {
        match receiver.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(record) => write(record),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if Instant::now() >= next {
            report(dropped.load(Ordering::Relaxed));
            next = Instant::now() + interval;
        }
    }
}
static WRITER: OnceLock<Option<Queue<Envelope>>> = OnceLock::new();

pub(in crate::store) fn initialize() -> bool {
    WRITER
        .get_or_init(|| {
            let dispatch = tracing::dispatcher::get_default(Clone::clone);
            Queue::spawn(
                CAPACITY,
                INTERVAL,
                |envelope: Envelope| {
                    tracing::dispatcher::with_default(&envelope.dispatch, || {
                        envelope
                            .record
                            .write(super::micros(envelope.queued_at.elapsed()))
                    });
                },
                move |dropped| {
                    tracing::dispatcher::with_default(&dispatch, || {
                        tracing::info!(target: "mongo_diagnostic_output", parent: None,
                    dropped_total = dropped, queue_capacity = CAPACITY,
                    "MongoDB diagnostic output loss");
                    });
                },
            )
            .ok()
        })
        .is_some()
}
pub(in crate::store) fn send(record: Record) {
    // Initialized before installing callbacks; no thread creation or formatting here.
    if let Some(Some(queue)) = WRITER.get() {
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        // An unsubscribed caller has no output destination. Do not register the
        // writer's callsites under NoSubscriber: tracing's single-dispatch cache
        // can otherwise suppress a later record from a scoped subscriber.
        if dispatch.is::<tracing::subscriber::NoSubscriber>() {
            return;
        }
        queue.enqueue(Envelope {
            record,
            dispatch,
            queued_at: Instant::now(),
        });
    }
}
impl Record {
    fn write(self, queue_delay_us: u64) {
        match self {
            Self::Detail {
                command,
                event,
                duration,
                docs,
                hold_us,
                deferred,
            } => command.write(event, duration, docs, hold_us, deferred, queue_delay_us),
            Self::Summary(row) => summary::write(row, queue_delay_us),
            Self::Latency {
                pool,
                operation,
                window_ms,
                count,
                failures,
                total_us,
                max_us,
                buckets,
                in_use,
                in_use_peak,
                untracked,
            } => {
                tracing::info!(target: "cma::store::mongo_latency", parent: None, queue_delay_us, pool, operation, window_ms, count, failures, total_us, max_us, buckets = ?buckets, in_use, in_use_peak, untracked, "MongoDB client latency summary");
            }
            Self::Circuit {
                operation,
                reason,
                elapsed_ms,
                failed,
            } => {
                tracing::warn!(target: "cma::store::usage_reports::budget", parent: None, queue_delay_us, operation, reason, elapsed_ms, failed, "sandbox reporting database circuit opened");
            }
            Self::Pool {
                pool,
                commands,
                holds,
                oldest_hold_us,
                untracked,
            } => {
                tracing::info!(target: "mongo_diagnostic_pool", parent: None,
                    queue_delay_us, pool, tracked_commands = commands, tracked_holds = holds,
                    oldest_tracked_hold_us = oldest_hold_us, untracked,
                    "MongoDB diagnostic heartbeat");
            }
            #[cfg(test)]
            Self::Barrier(done) => {
                let _ = done.send(());
            }
        }
    }
}
#[cfg(test)]
pub(in crate::store) fn flush() {
    let (done, received) = mpsc::channel();
    let Some(queue) = WRITER.get().and_then(Option::as_ref) else {
        return;
    };
    assert!(queue
        .sender
        .try_send(Envelope {
            record: Record::Barrier(done),
            dispatch: tracing::dispatcher::get_default(Clone::clone),
            queued_at: Instant::now(),
        })
        .is_ok());
    received.recv_timeout(Duration::from_secs(5)).unwrap();
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stalled_writer_and_full_queue_never_wait_and_count_each_drop() {
        let (entered, started) = mpsc::channel();
        let (release, resume) = mpsc::channel::<()>();
        let (reported, reports) = mpsc::channel();
        let queue = Arc::new(
            Queue::spawn(
                1,
                Duration::from_millis(10),
                move |_: u8| {
                    let _ = entered.send(());
                    let _ = resume.recv();
                },
                move |count| {
                    let _ = reported.send(count);
                },
            )
            .unwrap(),
        );
        queue.enqueue(1);
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        queue.enqueue(2);
        let producer = queue.clone();
        let (done, completed) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            for _ in 0..10_000 {
                producer.enqueue(3);
            }
            done.send(()).unwrap();
        });
        let result = completed.recv_timeout(Duration::from_secs(1));
        // Unblock before assertions so a failure cannot leave a stalled fixture.
        drop(release);
        assert!(result.is_ok(), "full queue blocked the callback path");
        thread.join().unwrap();
        assert_eq!(queue.dropped.load(Ordering::Relaxed), 10_000);
        assert_eq!(
            reports.recv_timeout(Duration::from_secs(1)).unwrap(),
            10_000
        );
    }
    #[test]
    fn normal_records_are_delivered_and_disconnected_writer_counts_loss() {
        let (sent, received) = mpsc::channel();
        let queue =
            Queue::spawn(2, INTERVAL, move |value| sent.send(value).unwrap(), |_| {}).unwrap();
        queue.enqueue(7);
        assert_eq!(received.recv_timeout(Duration::from_secs(1)).unwrap(), 7);
        assert_eq!(queue.dropped.load(Ordering::Relaxed), 0);
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        let queue = Queue {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
        };
        queue.enqueue(1);
        assert_eq!(queue.dropped.load(Ordering::Relaxed), 1);
    }
}
