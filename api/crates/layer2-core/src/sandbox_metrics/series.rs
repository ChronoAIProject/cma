//! Time-weighted observations and physical sums are distinct operations.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bucket {
    pub from: u64,
    pub to: u64,
    /// Actual counter delta or gauge integral, used for period totals/ranking.
    pub integral: f64,
    pub display_integral: f64,
    pub capacity_usage_integral: f64,
    pub capacity_integral: f64,
    pub covered_seconds: f64,
    pub expected_seconds: f64,
    pub unknown_mapping_seconds: f64,
    pub mean: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub excluded_count: u64,
}
impl Bucket {
    /// Merge disjoint times of the SAME Sandbox; retain its peak envelope.
    pub fn merge_time(&mut self, other: &Self) {
        self.from = self.from.min(other.from);
        self.to = self.to.max(other.to);
        self.add_statistics(other);
        self.min = model::option_min(self.min, other.min);
        self.max = model::option_max(self.max, other.max);
        self.finish_time();
    }
    pub fn finish_time(&mut self) {
        self.mean =
            (self.covered_seconds > 0.).then(|| self.display_integral / self.covered_seconds);
        self.excluded_count = u64::from(
            self.covered_seconds + 0.001 < self.expected_seconds
                || self.unknown_mapping_seconds > 0.,
        );
    }
    /// Sum DIFFERENT Sandboxes after each has a time-weighted mean. Min/max
    /// are a conservative sum of observed envelopes, not concurrent peaks.
    pub fn merge(&mut self, other: &Self) {
        self.add_statistics(other);
        self.mean = sum(self.mean, other.mean);
        self.min = sum(self.min, other.min);
        self.max = sum(self.max, other.max);
        self.excluded_count += other.excluded_count;
    }
    fn add_statistics(&mut self, other: &Self) {
        self.integral += other.integral;
        self.display_integral += other.display_integral;
        self.capacity_usage_integral += other.capacity_usage_integral;
        self.capacity_integral += other.capacity_integral;
        self.covered_seconds += other.covered_seconds;
        self.expected_seconds += other.expected_seconds;
        self.unknown_mapping_seconds += other.unknown_mapping_seconds;
    }
    pub fn point(self) -> Point {
        Point {
            from: self.from,
            to: self.to,
            mean: self.mean,
            min: self.min,
            max: self.max,
            covered_seconds: self.covered_seconds,
            expected_seconds: self.expected_seconds,
            unknown_mapping_seconds: self.unknown_mapping_seconds,
            numerator: (self.capacity_integral > 0.).then_some(self.capacity_usage_integral),
            denominator: (self.capacity_integral > 0.).then_some(self.capacity_integral),
            reason: if self.mean.is_none() {
                Some(
                    if self.expected_seconds == 0. && self.unknown_mapping_seconds == 0. {
                        "not_applicable"
                    } else {
                        "no_measurement"
                    }
                    .into(),
                )
            } else if self.excluded_count > 0 {
                Some("partial_coverage".into())
            } else {
                None
            },
            excluded_count: self.excluded_count,
        }
    }
}
pub fn sum(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    }
}
pub fn buckets(minutes: &[Minute], metric: Metric, range: Range, step: u64) -> Vec<Bucket> {
    let mut out = BTreeMap::new();
    let mut at = range.from / step * step;
    while at < range.to && range.seconds() > 0 {
        out.insert(
            at,
            Bucket {
                from: at.max(range.from),
                to: (at + step).min(range.to),
                ..Default::default()
            },
        );
        at += step;
    }
    for minute in minutes {
        if minute.to <= range.from || minute.from >= range.to {
            continue;
        }
        if let Some(bucket) = out.get_mut(&(minute.from / step * step)) {
            let v = &minute.metrics[metric.index()];
            bucket.integral += v.integral;
            bucket.display_integral += v.display_integral;
            bucket.capacity_usage_integral += v.capacity_usage_integral;
            bucket.capacity_integral += v.capacity_integral;
            bucket.covered_seconds += v.covered_seconds;
            bucket.expected_seconds += v.expected_seconds;
            bucket.min = model::option_min(bucket.min, v.min);
            bucket.max = model::option_max(bucket.max, v.max);
        }
    }
    for v in out.values_mut() {
        v.finish_time();
    }
    out.into_values().collect()
}
/// Reconstruct explicit null buckets from compact allocation intervals. This
/// preserves unknown historical identity even when the source returned nothing.
pub fn complete(
    sparse: &BTreeMap<u64, Bucket>,
    expectations: &[allocation::Expectation],
    metric: Metric,
    range: Range,
    step: u64,
) -> Vec<Bucket> {
    let mut out = Vec::new();
    complete_into(&mut out, sparse, expectations, metric, range, step);
    out
}

/// Reuse the caller's allocation when folding multiple Sandboxes on one grid.
pub fn complete_into(
    out: &mut Vec<Bucket>,
    sparse: &BTreeMap<u64, Bucket>,
    expectations: &[allocation::Expectation],
    metric: Metric,
    range: Range,
    step: u64,
) {
    out.clear();
    if range.seconds() == 0 {
        return;
    }
    let mut sparse = sparse.iter();
    let mut measured = sparse.next();
    let uniform = match expectations {
        [interval] if interval.from <= range.from && interval.to >= range.to => {
            Some(if metric == Metric::Disk {
                interval.home
            } else {
                interval.running
            })
        }
        _ => None,
    };
    let mut at = range.from / step * step;
    let mut first = 0;
    while at < range.to {
        let from = at.max(range.from);
        let to = (at + step).min(range.to);
        while measured.is_some_and(|(key, _)| *key < from) {
            measured = sparse.next();
        }
        let mut b = match measured {
            Some((&key, value)) if key == from => {
                measured = sparse.next();
                value.clone()
            }
            _ if uniform.is_some() => {
                // A missing sample over one covering interval has neither
                // measured statistics nor any other interval to intersect.
                let duration = (to - from) as f64;
                let expected = uniform == Some(Some(true));
                let excluded = uniform != Some(Some(false));
                out.push(Bucket {
                    from,
                    to,
                    expected_seconds: if expected { duration } else { 0. },
                    unknown_mapping_seconds: if excluded { duration } else { 0. },
                    excluded_count: u64::from(excluded),
                    ..Default::default()
                });
                at += step;
                continue;
            }
            _ => Bucket {
                from,
                to,
                ..Default::default()
            },
        };
        while first < expectations.len() && expectations[first].to <= b.from {
            first += 1;
        }
        let mut expected = 0.;
        let mut unknown = 0.;
        for e in expectations[first..].iter().take_while(|e| e.from < b.to) {
            let duration = e.to.min(b.to).saturating_sub(e.from.max(b.from)) as f64;
            match if metric == Metric::Disk {
                e.home
            } else {
                e.running
            } {
                Some(true) => expected += duration,
                Some(false) => {}
                None => unknown += duration,
            }
        }
        b.unknown_mapping_seconds = (expected - b.expected_seconds).max(0.) + unknown;
        b.expected_seconds = b.expected_seconds.max(expected);
        b.finish_time();
        out.push(b);
        at += step;
    }
}
