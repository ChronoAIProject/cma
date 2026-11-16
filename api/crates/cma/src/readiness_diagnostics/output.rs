//! Same bounded try_send/dedicated-writer pattern as Mongo diagnostics (#1769).
//! Snapshot storage has a separate two-item lane so event volume cannot grow it.
use super::{micros, record::Record, ring::Snapshot, Event};
use std::{
    io,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    time::{Duration, Instant},
};

pub(super) const RECORD_CAPACITY: usize = 512;
pub(super) const SNAPSHOT_CAPACITY: usize = 2;
pub(super) enum Message {
    Record(Record),
    Event(Arc<Event>),
}
pub(super) struct Envelope<T> {
    pub process: Arc<str>,
    pub item: T,
    pub queued: Instant,
}
pub(super) struct Output {
    records: SyncSender<Envelope<Message>>,
    snapshots: SyncSender<Envelope<Snapshot>>,
    events: SyncSender<Envelope<Message>>,
    losses: Arc<AtomicU64>,
}
pub(super) struct Inputs {
    pub records: Receiver<Envelope<Message>>,
    pub snapshots: Receiver<Envelope<Snapshot>>,
    pub events: Receiver<Envelope<Message>>,
}

impl Output {
    #[cfg(test)]
    pub(super) fn loss_count(&self) -> u64 {
        self.losses.load(Ordering::Relaxed)
    }
    pub(super) fn channel() -> (Self, Inputs) {
        let (records, receive) = mpsc::sync_channel(RECORD_CAPACITY);
        let (snapshots, frozen) = mpsc::sync_channel(SNAPSHOT_CAPACITY);
        let (events, observed) = mpsc::sync_channel(RECORD_CAPACITY);
        (
            Self {
                records,
                snapshots,
                events,
                losses: Arc::new(AtomicU64::new(0)),
            },
            Inputs {
                records: receive,
                snapshots: frozen,
                events: observed,
            },
        )
    }
    pub(super) fn start() -> io::Result<Self> {
        let (output, inputs) = Self::channel();
        let losses = output.losses.clone();
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        std::thread::Builder::new()
            .name("readiness-diagnostic-writer".into())
            .spawn(move || {
                tracing::dispatcher::with_default(&dispatch, || inputs.drain(&losses));
            })?;
        Ok(output)
    }
    fn failed(&self, reason: &'static str) {
        self.losses.fetch_add(1, Ordering::Relaxed);
        // Independent metrics remain visible even while the log sink is stalled.
        metrics::counter!("cma_readiness_diagnostic_loss_total", "reason" => reason).increment(1);
    }
    pub(super) fn record(&self, process: Arc<str>, record: Record) {
        self.send(process, Message::Record(record));
    }
    pub(super) fn event(&self, process: Arc<str>, event: Arc<Event>) {
        if self
            .events
            .try_send(Envelope {
                process,
                item: Message::Event(event),
                queued: Instant::now(),
            })
            .is_err()
        {
            self.failed("event_queue");
        }
    }
    fn send(&self, process: Arc<str>, item: Message) {
        if self
            .records
            .try_send(Envelope {
                process,
                item,
                queued: Instant::now(),
            })
            .is_err()
        {
            self.failed("record_queue");
        }
    }
    pub(super) fn snapshot(&self, process: Arc<str>, item: Snapshot) {
        if self
            .snapshots
            .try_send(Envelope {
                process,
                item,
                queued: Instant::now(),
            })
            .is_err()
        {
            self.failed("snapshot_queue");
        }
    }
}

impl Inputs {
    fn drain(self, losses: &AtomicU64) {
        let mut reported = Instant::now();
        loop {
            match self.records.recv_timeout(Duration::from_millis(10)) {
                Ok(envelope) => envelope.write(),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            for envelope in self.events.try_iter().take(64) {
                envelope.write();
            }
            if let Ok(snapshot) = self.snapshots.try_recv() {
                let delay = micros(snapshot.queued.elapsed());
                metrics::gauge!("cma_readiness_diagnostic_queue_delay_seconds")
                    .set(delay as f64 / 1e6);
                snapshot.item.write(&snapshot.process, delay);
            }
            if reported.elapsed() >= Duration::from_secs(10) {
                tracing::info!(target: "readiness_diagnostic_output", parent: None,
                    dropped_total=losses.load(Ordering::Relaxed), record_capacity=RECORD_CAPACITY,
                    snapshot_capacity=SNAPSHOT_CAPACITY, event_capacity=RECORD_CAPACITY, "Readiness diagnostic output heartbeat");
                reported = Instant::now();
            }
        }
    }
}

impl Envelope<Message> {
    pub(super) fn write(self) {
        let delay = micros(self.queued.elapsed());
        metrics::gauge!("cma_readiness_diagnostic_queue_delay_seconds").set(delay as f64 / 1e6);
        match self.item {
            Message::Record(record) => record.write(&self.process, delay),
            Message::Event(event) => event.write(&self.process, 0, delay),
        }
    }
}
