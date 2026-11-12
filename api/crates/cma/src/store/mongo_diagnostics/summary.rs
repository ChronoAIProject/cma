//! Unsampled totals. Sum hold_us only from metric=hold, never command events.
use super::{micros, Command};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
const MAX_GROUPS: usize = 1024;
#[derive(Hash, PartialEq, Eq)]
struct Key {
    pool: &'static str,
    route: Arc<str>,
    worker: &'static str,
    phase: &'static str,
    command: &'static str,
    collection: &'static str,
    metric: &'static str,
}
#[derive(Default)]
struct Totals {
    count: u64,
    total_us: u64,
    max_us: u64,
    docs: u64,
    failures: u64,
}
pub(super) struct Summary {
    start: Instant,
    groups: HashMap<Key, Totals>,
    pub(super) untracked: u64,
}
impl Default for Summary {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            groups: HashMap::new(),
            untracked: 0,
        }
    }
}
pub(super) struct Completed {
    window: Summary,
    elapsed: Duration,
}
impl Summary {
    pub(super) fn record(
        &mut self,
        command: &Command,
        metric: &'static str,
        duration: Duration,
        docs: Option<u64>,
        failed: bool,
    ) -> Option<Completed> {
        let mut key = Key {
            pool: command.pool,
            route: command.context.route.clone(),
            worker: command.context.worker,
            phase: command.context.phase,
            command: command.command,
            collection: command.collection,
            metric,
        };
        if self.groups.len() >= MAX_GROUPS && !self.groups.contains_key(&key) {
            key = Key {
                pool: command.pool,
                route: "overflow".into(),
                worker: "overflow",
                phase: "overflow",
                command: "overflow",
                collection: "overflow",
                metric,
            };
        }
        let value = self.groups.entry(key).or_default();
        value.count = value.count.saturating_add(1);
        value.total_us = value.total_us.saturating_add(micros(duration));
        value.max_us = value.max_us.max(micros(duration));
        value.docs = value.docs.saturating_add(docs.unwrap_or(0));
        value.failures = value.failures.saturating_add(u64::from(failed));
        let elapsed = self.start.elapsed();
        (elapsed >= Duration::from_secs(10)).then(|| Completed {
            window: std::mem::take(self),
            elapsed,
        })
    }
}
pub(super) fn emit(completed: Option<Completed>) {
    if let Some(completed) = completed {
        for (key, value) in completed.window.groups {
            tracing::info!(target: "mongo_diagnostic_summary", parent: None,
                pool = key.pool, route = %key.route, worker = key.worker, phase = key.phase,
                command = key.command, collection = key.collection, metric = key.metric,
                count = value.count, total_us = value.total_us, max_us = value.max_us,
                docs_returned = value.docs, failures = value.failures,
                window_us = micros(completed.elapsed), untracked = completed.window.untracked,
                "MongoDB attributed totals");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;
    #[test]
    fn all_fast_work_is_counted_and_request_hashes_do_not_expand_cardinality() {
        let mut summary = Summary::default();
        let mut command = super::super::metadata("find", &doc! {"find":"agents"}, false);
        for i in 0..1000 {
            command.context.request_hash = format!("{i:064x}").into();
            assert!(summary
                .record(&command, "hold", Duration::from_micros(10), None, false)
                .is_none());
        }
        assert_eq!(summary.groups.len(), 1);
        let value = summary.groups.values().next().unwrap();
        assert_eq!(
            (value.count, value.total_us, value.max_us),
            (1000, 10_000, 10)
        );
        summary.start = Instant::now() - Duration::from_secs(11);
        let completed = summary
            .record(&command, "hold", Duration::from_micros(20), None, true)
            .unwrap();
        assert!(summary.groups.is_empty());
        let value = completed.window.groups.values().next().unwrap();
        assert_eq!(
            (value.count, value.total_us, value.failures),
            (1001, 10_020, 1)
        );
    }
    #[test]
    fn group_overflow_is_bounded_without_losing_duration_totals() {
        let mut summary = Summary::default();
        let mut command = super::super::metadata("find", &doc! {"find":"agents"}, false);
        for i in 0..5000 {
            command.context.route = format!("route-{i}").into();
            summary.record(&command, "hold", Duration::from_micros(1), None, false);
        }
        assert_eq!(summary.groups.len(), MAX_GROUPS + 1);
        assert_eq!(
            summary.groups.values().map(|v| v.total_us).sum::<u64>(),
            5000
        );
        assert_eq!(
            summary
                .groups
                .iter()
                .find(|(k, _)| k.route.as_ref() == "overflow")
                .unwrap()
                .1
                .count,
            5000 - MAX_GROUPS as u64
        );
    }
}
