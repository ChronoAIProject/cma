//! Unsampled totals. Sum hold_us only from metric=hold, never command events.
use super::{micros, Command};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
const MAX_GROUPS: usize = 1024;
// Fixed inclusive bounds preserve the 25 ms decision threshold without keeping
// samples or introducing labels. The final bucket is strictly above the last bound.
const DURATION_BOUNDS_US: [u64; 10] = [
    1_000, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000, 500_000, 1_000_000, 5_000_000,
];
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
    docs_known: u64,
    outcomes_unknown: u64,
    duration_buckets: [u64; DURATION_BOUNDS_US.len() + 1],
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
    pub(super) fn drain(&mut self) -> Completed {
        let elapsed = self.start.elapsed();
        Completed {
            window: std::mem::take(self),
            elapsed,
        }
    }
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
        // Compare Duration directly: truncating 25 ms + 1 ns to microseconds
        // would incorrectly count an observation above the acceptance boundary.
        let bucket =
            DURATION_BOUNDS_US.partition_point(|bound| duration > Duration::from_micros(*bound));
        value.duration_buckets[bucket] = value.duration_buckets[bucket].saturating_add(1);
        value.docs = value.docs.saturating_add(docs.unwrap_or(0));
        value.docs_known = value.docs_known.saturating_add(u64::from(docs.is_some()));
        value.outcomes_unknown = value
            .outcomes_unknown
            .saturating_add(u64::from(metric == "command" && !command.outcome_known));
        value.failures = value.failures.saturating_add(u64::from(failed));
        let elapsed = self.start.elapsed();
        (elapsed >= Duration::from_secs(10)).then(|| self.drain())
    }
}
pub(in crate::store) struct Row {
    key: Key,
    value: Totals,
    elapsed: Duration,
    untracked: u64,
}
pub(super) fn emit(completed: Option<Completed>) {
    if let Some(completed) = completed {
        for (key, value) in completed.window.groups {
            super::output::send(super::output::Record::Summary(Row {
                key,
                value,
                elapsed: completed.elapsed,
                untracked: completed.window.untracked,
            }));
        }
    }
}
pub(super) fn write(row: Row, queue_delay_us: u64) {
    let Row {
        key,
        value,
        elapsed,
        untracked,
    } = row;
    tracing::info!(target: "mongo_diagnostic_summary", parent: None,
        queue_delay_us, pool = key.pool, route = key.route.as_ref(), worker = key.worker, phase = key.phase,
        command = key.command, collection = key.collection, metric = key.metric,
        count = value.count, total_us = value.total_us, max_us = value.max_us,
        duration_bucket_bounds_us = ?DURATION_BOUNDS_US, duration_bucket_counts = ?value.duration_buckets,
        docs_returned = value.docs, docs_returned_known_count = value.docs_known, outcomes_unknown = value.outcomes_unknown, failures = value.failures,
        window_us = micros(elapsed), untracked,
        "MongoDB attributed totals");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_fast_work_is_counted_and_request_hashes_do_not_expand_cardinality() {
        let mut summary = Summary::default();
        let mut command = super::super::metadata("find", "agents", false, false);
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
        let mut command = super::super::metadata("find", "agents", false, false);
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
    #[test]
    fn duration_buckets_are_inclusive_unsampled_and_do_not_round_down() {
        let command = super::super::metadata("findAndModify", "request_rate_limits", false, false);
        for metric in ["command", "checkout", "hold"] {
            let mut summary = Summary::default();
            let mut expected = [0u64; DURATION_BOUNDS_US.len() + 1];
            summary.record(&command, metric, Duration::ZERO, None, false);
            expected[0] += 1;
            for (index, bound) in DURATION_BOUNDS_US.iter().enumerate() {
                let duration = Duration::from_micros(*bound);
                for (sample, bucket) in [
                    (duration - Duration::from_nanos(1), index),
                    (duration, index),
                    (duration + Duration::from_nanos(1), index + 1),
                ] {
                    summary.record(&command, metric, sample, None, true);
                    expected[bucket] += 1;
                }
            }
            let totals = summary.groups.values().next().unwrap();
            assert_eq!(totals.duration_buckets, expected);
            assert_eq!(totals.duration_buckets.iter().sum::<u64>(), totals.count);
            assert_eq!(totals.failures, 30);
            let completed = summary.drain();
            assert!(summary.groups.is_empty());
            assert_eq!(
                completed
                    .window
                    .groups
                    .values()
                    .next()
                    .unwrap()
                    .duration_buckets,
                expected
            );
        }
    }

    #[test]
    fn duration_buckets_survive_overflow_and_saturate_without_wrapping() {
        let mut summary = Summary::default();
        let mut command = super::super::metadata("find", "auth_subjects", false, false);
        for index in 0..MAX_GROUPS + 10 {
            command.context.route = format!("route-{index}").into();
            summary.record(&command, "command", Duration::from_millis(25), None, false);
        }
        let completed = summary.drain();
        assert_eq!(completed.window.groups.len(), MAX_GROUPS + 1);
        assert_eq!(
            completed
                .window
                .groups
                .values()
                .map(|v| v.duration_buckets[3])
                .sum::<u64>(),
            (MAX_GROUPS + 10) as u64
        );
        summary.record(&command, "command", Duration::from_millis(25), None, false);
        let totals = summary.groups.values_mut().next().unwrap();
        totals.count = u64::MAX;
        totals.duration_buckets[3] = u64::MAX;
        summary.record(&command, "command", Duration::from_millis(25), None, false);
        let totals = summary.groups.values().next().unwrap();
        assert_eq!(totals.count, u64::MAX);
        assert_eq!(totals.duration_buckets[3], u64::MAX);
    }

    #[test]
    fn emitted_summary_exposes_fixed_bounds_and_counts_without_private_values() {
        let output = super::super::tests::Output::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_current_span(false)
            .with_span_list(false)
            .with_writer(output.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        assert!(super::super::output::initialize());
        let mut command = super::super::metadata("find", "auth_subjects", false, false);
        command.context.request_hash = "private-request-correlation".into();
        let mut summary = Summary::default();
        summary.record(&command, "command", Duration::from_millis(25), None, true);
        emit(Some(summary.drain()));
        super::super::output::flush();
        let bytes = output.0.lock().unwrap().clone();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("private-request-correlation"));
        let event: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        let fields = &event["fields"];
        let bounds: Vec<u64> =
            serde_json::from_str(fields["duration_bucket_bounds_us"].as_str().unwrap()).unwrap();
        let counts: Vec<u64> =
            serde_json::from_str(fields["duration_bucket_counts"].as_str().unwrap()).unwrap();
        assert_eq!(bounds, DURATION_BOUNDS_US);
        assert_eq!(counts.len(), bounds.len() + 1);
        assert_eq!(counts[3], 1);
        assert_eq!(
            counts.iter().sum::<u64>(),
            fields["count"].as_u64().unwrap()
        );
        assert_eq!(fields["failures"], 1);
    }
}
