use super::{micros, Event, Stamp, HISTORY};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

pub(super) const CAPACITY: usize = 8192;
const ACTIVE_CAPACITY: usize = 4096;

#[derive(Default)]
pub(super) struct Ring {
    rows: VecDeque<Arc<Event>>,
    holds: HashMap<(&'static str, Arc<str>), Arc<Event>>,
    commands: HashMap<u64, Arc<Event>>,
    evicted: u64,
    active_overflow: u64,
}

pub(crate) struct Snapshot {
    pub stamp: Stamp,
    pub check: u64,
    pub rows: Vec<Arc<Event>>,
    pub active: Vec<Arc<Event>>,
    pub evicted: u64,
    pub active_overflow: u64,
    pub lost: u64,
}

impl Ring {
    fn expire(&mut self, at: u64) {
        let left = at.saturating_sub(micros(HISTORY));
        while self.rows.front().is_some_and(|r| r.stamp.mono_us < left) {
            self.rows.pop_front();
        }
    }

    pub(super) fn push(&mut self, row: Arc<Event>) {
        self.expire(self.rows.back().map_or(row.stamp.mono_us, |r| {
            r.stamp.mono_us.max(row.stamp.mono_us)
        }));
        let m = &row.metadata;
        let key = (m.pool, m.hold_hash.clone());
        match row.event {
            "checked_out" if m.hold_hash.as_ref() != "none" => {
                if self.holds.len() < ACTIVE_CAPACITY {
                    self.holds.insert(key, row.clone());
                } else {
                    self.active_overflow += 1;
                    metrics::counter!("cma_readiness_diagnostic_loss_total", "reason" => "active_capacity").increment(1);
                }
            }
            "checked_in" | "connection_closed" => {
                self.holds.remove(&key);
                self.commands.retain(|_, r| {
                    !(r.metadata.pool == m.pool && r.metadata.hold_hash == m.hold_hash)
                });
            }
            "command_started" => {
                if self.commands.len() < ACTIVE_CAPACITY {
                    self.commands.insert(m.operation, row.clone());
                } else {
                    self.active_overflow += 1;
                    metrics::counter!("cma_readiness_diagnostic_loss_total", "reason" => "active_capacity").increment(1);
                }
            }
            "command_succeeded" | "command_failed" | "command_completed" => {
                self.commands.remove(&m.operation);
            }
            _ => {}
        }
        if self.rows.len() == CAPACITY {
            self.rows.pop_front();
            self.evicted += 1;
            metrics::counter!("cma_readiness_diagnostic_loss_total", "reason" => "ring_capacity")
                .increment(1);
        }
        // Driver callbacks on different threads can arrive out of timestamp order.
        let index = self
            .rows
            .partition_point(|r| r.stamp.mono_us <= row.stamp.mono_us);
        self.rows.insert(index, row);
    }

    pub(super) fn freeze(&mut self, stamp: Stamp, check: u64, lost: u64) -> Snapshot {
        self.expire(stamp.mono_us);
        Snapshot {
            stamp,
            check,
            rows: self.rows.iter().cloned().collect(),
            active: self
                .holds
                .values()
                .chain(self.commands.values())
                .cloned()
                .collect(),
            evicted: self.evicted,
            active_overflow: self.active_overflow,
            lost,
        }
    }
}

impl Snapshot {
    pub(super) fn write(self, process: &str, queue_delay_us: u64) {
        tracing::info!(target: "readiness_diagnostic_snapshot", parent: None,
            process_hash=process, check_sequence=self.check, observed_unix_us=self.stamp.unix_us,
            process_monotonic_us=self.stamp.mono_us, event_sequence=self.stamp.sequence,
            window_start_us=self.stamp.mono_us.saturating_sub(micros(HISTORY)), window_end_us=self.stamp.mono_us,
            rows=self.rows.len(), active_rows=self.active.len(), capacity=CAPACITY,
            capacity_evictions_total=self.evicted, active_overflow_total=self.active_overflow,
            capture_loss_total=self.lost, queue_delay_us, "Readiness deadline snapshot");
        for row in &self.rows {
            row.write(process, self.check, queue_delay_us);
        }
        for row in &self.active {
            tracing::info!(target: "readiness_diagnostic_snapshot", parent: None,
                process_hash=process, check_sequence=self.check, event_sequence=row.stamp.sequence,
                active_age_us=self.stamp.mono_us.saturating_sub(row.stamp.mono_us),
                operation_sequence=row.metadata.operation, hold_hash=%row.metadata.hold_hash,
                pool=row.metadata.pool, worker=row.metadata.context.worker,
                route=row.metadata.context.route.as_ref(), phase=row.metadata.context.phase,
                command=row.metadata.command, collection=row.metadata.collection,
                event=row.event, "Readiness outstanding Mongo work");
        }
    }
}
