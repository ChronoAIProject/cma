use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use std::time::Duration;

use super::{
    clock::Clock,
    gate::Gate,
    runner::{Cell, SharedExecutionRunner, Termination},
};

struct Entry {
    cell: Cell,
    gate: Arc<Gate>,
    cancelled_at: Option<u64>,
    forced: bool,
}

pub(super) struct Watchdog {
    entries: Arc<Mutex<Vec<Entry>>>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    pub(super) fn start(
        clock: Arc<dyn Clock>,
        runner: Arc<dyn SharedExecutionRunner>,
    ) -> std::io::Result<Self> {
        let entries = Arc::new(Mutex::new(Vec::<Entry>::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let watched = entries.clone();
        let stop = stopping.clone();
        let thread = std::thread::Builder::new()
            .name("resource-watchdog".into())
            .spawn(move || {
                loop {
                    let stopping = stop.load(Ordering::Acquire);
                    let tick = clock.tick_ms();
                    if let Ok(mut entries) = watched.lock() {
                        for entry in entries.iter_mut() {
                            if stopping || tick.is_err() {
                                entry.gate.close();
                            }
                            if !entry.gate.expire(tick.unwrap_or(u64::MAX)) {
                                continue;
                            }
                            let now = tick.unwrap_or(u64::MAX);
                            if entry.cancelled_at.is_none() {
                                entry.cancelled_at = Some(now);
                                if runner
                                    .cancel(&entry.cell, Termination::Cooperative)
                                    .is_err()
                                {
                                    let _ = runner.cancel(&entry.cell, Termination::Force);
                                }
                            }
                            // 1.90 local seconds bounds the real cooperative grace
                            // under the qualification profile's minimum clock rate.
                            if !entry.forced
                                && (stopping
                                    || tick.is_err()
                                    || now.saturating_sub(entry.cancelled_at.unwrap_or(now))
                                        >= 1900)
                            {
                                entry.forced =
                                    runner.cancel(&entry.cell, Termination::Force).is_ok();
                            }
                        }
                    }
                    if stopping {
                        return;
                    }
                    std::thread::park_timeout(Duration::from_millis(25));
                }
            })?;
        Ok(Self {
            entries,
            stopping,
            thread: Some(thread),
        })
    }

    pub(super) fn register(&self, cell: Cell, gate: Arc<Gate>) -> Result<(), ()> {
        let mut entries = self.entries.lock().map_err(|_| ())?;
        if entries.len() >= super::journal::MAX_RECORDS {
            return Err(());
        }
        if entries.iter().any(|entry| entry.cell == cell) {
            return Err(());
        }
        entries.push(Entry {
            cell,
            gate,
            cancelled_at: None,
            forced: false,
        });
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
        Ok(())
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}
