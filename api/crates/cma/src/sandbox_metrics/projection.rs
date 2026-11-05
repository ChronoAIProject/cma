use crate::store::{sandbox_metric_rollups::LiveReading, usage_reports::inventory::Inventory};
use layer2_core::sandbox_metrics::*;
use std::collections::BTreeMap;

pub(super) struct Accumulator {
    pub allocation: AllocatedPeriod,
    pub expectations: Vec<allocation::Expectation>,
    pub metrics: [Aggregate; 3],
    pub buckets: [BTreeMap<u64, series::Bucket>; 3],
}
impl Default for Accumulator {
    fn default() -> Self {
        Self {
            allocation: AllocatedPeriod {
                complete: true,
                ..Default::default()
            },
            expectations: Vec::new(),
            metrics: Default::default(),
            buckets: Default::default(),
        }
    }
}
impl Accumulator {
    pub fn add(&mut self, minutes: &[Minute], query: &ReportQuery, range: Range) {
        for (metric, total) in [Metric::Cpu, Metric::Memory, Metric::Disk]
            .into_iter()
            .zip(&mut self.metrics)
        {
            for minute in minutes {
                total.merge(&minute.metrics[metric.index()]);
            }
            for b in series::buckets(minutes, metric, range, query.step_seconds) {
                if b.covered_seconds == 0. && b.expected_seconds == 0. {
                    continue;
                }
                self.buckets[metric.index()]
                    .entry(b.from)
                    .and_modify(|v| v.merge_time(&b))
                    .or_insert(b);
            }
        }
    }
    pub fn row(&self, item: &Inventory, live: Option<&LiveReading>, report: &Report) -> Row {
        let state = &item.state;
        let measurements = [Metric::Cpu, Metric::Memory, Metric::Disk].map(|metric| {
            let mut a = self.metrics[metric.index()].clone();
            let expected = if metric == Metric::Disk {
                self.allocation.disk_retained_seconds
            } else {
                self.allocation.uptime_seconds
            };
            let unknown =
                (expected - a.expected_seconds).max(0.) + self.allocation.unknown_seconds as f64;
            a.expected_seconds = a.expected_seconds.max(expected);
            if let Some(live) = live.filter(|l| l.owner == state.owner.as_str()) {
                let current = &live.metrics[metric.index()];
                if current.observed_at > a.observed_at
                    && current
                        .observed_at
                        .is_some_and(|t| t <= report.as_of as f64)
                {
                    a.observed_at = current.observed_at;
                    a.latest = current.latest;
                    a.latest_capacity = current.latest_capacity;
                }
            }
            Measurement::project(&a, metric, report.as_of, state.allocation.running, unknown)
        });
        let [cpu, memory, disk] = measurements;
        Row {
            sandbox_id: state.sandbox_id.to_string(),
            name: state.name.clone(),
            owner: Owner {
                subject: state.owner.to_string(),
                display_name: None,
            },
            status: state.status.clone(),
            runtime_version: item
                .sandbox
                .as_ref()
                .and_then(|s| {
                    s.runtime
                        .as_ref()
                        .or(s.lifecycle.suspended_runtime.as_deref())
                })
                .map(|r| r.runtime_version.clone()),
            agent_count: item.agent_count,
            last_activity_at: item
                .sandbox
                .as_ref()
                .and_then(|s| layer2_core::seconds_from_rfc3339(&s.last_activity_at)),
            allocation: Capacity {
                cpu_cores: state.allocation.cpu_millis as f64 / 1000.,
                memory_bytes: state.allocation.memory_mib as f64 * 1_048_576.,
                disk_bytes: state.allocation.disk_gib as f64 * 1_073_741_824.,
            },
            allocated_period: self.allocation.clone(),
            measured: Measured { cpu, memory, disk },
            billing: Billing::default(),
            shared_sandbox_usage: report.query.filters.agent_id.is_some(),
        }
    }
}
pub(super) fn add_summary(
    summary: &mut Summary,
    row: &Row,
    aggregate: &mut super::summary::Fleet,
    value: &Accumulator,
) {
    summary.sandbox_count += 1;
    summary.running_count += u64::from(row.measured.cpu.state != MetricState::NotRunning);
    summary.allocated_period.merge(&row.allocated_period);
    summary.total_allocated_capacity.cpu_cores += row.allocation.cpu_cores;
    summary.total_allocated_capacity.memory_bytes += row.allocation.memory_bytes;
    summary.total_allocated_capacity.disk_bytes += row.allocation.disk_bytes;
    if row.measured.cpu.latest.is_some() {
        summary.covered_allocated_capacity.cpu_cores += row.allocation.cpu_cores;
    }
    if row.measured.memory.latest.is_some() {
        summary.covered_allocated_capacity.memory_bytes += row.allocation.memory_bytes;
    }
    if row.measured.disk.latest.is_some() {
        summary.covered_allocated_capacity.disk_bytes += row.allocation.disk_bytes;
    }
    summary.excluded_count += u64::from(
        [&row.measured.cpu, &row.measured.memory, &row.measured.disk]
            .iter()
            .any(|m| m.coverage_ratio.is_none_or(|v| v < 0.999) || m.unknown_mapping_seconds > 0.),
    );
    aggregate.add(row, &value.metrics);
}
