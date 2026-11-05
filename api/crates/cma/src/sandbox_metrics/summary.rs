//! Fleet values sum Sandbox projections; time statistics retain their units.
use layer2_core::sandbox_metrics::{model, series, *};

#[derive(Default)]
pub(super) struct Fleet {
    pub totals: [Aggregate; 3],
    means: [Option<f64>; 3],
    peaks: [Option<f64>; 3],
    latest: [Option<f64>; 3],
    last_observed: [Option<f64>; 3],
    oldest_sample: [Option<f64>; 3],
    unknown: [f64; 3],
    filesystem_capacity: Option<f64>,
}
impl Fleet {
    pub fn add(&mut self, row: &Row, statistics: &[Aggregate; 3]) {
        for (i, m) in [&row.measured.cpu, &row.measured.memory, &row.measured.disk]
            .into_iter()
            .enumerate()
        {
            self.totals[i].merge(&statistics[i]);
            // The row includes allocation-derived unknown intervals and latest
            // live readings, which cannot be inferred from finalized rollups.
            self.totals[i].expected_seconds += m.expected_seconds - statistics[i].expected_seconds;
            self.means[i] = series::sum(self.means[i], m.mean);
            self.peaks[i] = series::sum(self.peaks[i], m.max);
            self.latest[i] = series::sum(self.latest[i], m.latest);
            self.last_observed[i] = series::sum(self.last_observed[i], m.last_observed);
            self.oldest_sample[i] = model::option_min(self.oldest_sample[i], m.observed_at);
            self.unknown[i] += m.unknown_mapping_seconds;
        }
        if row.measured.disk.latest.is_some() {
            self.filesystem_capacity = series::sum(
                self.filesystem_capacity,
                row.measured.disk.filesystem_capacity_bytes,
            );
        }
    }
    pub fn measured(&self, at: u64, running: bool) -> Measured {
        let [cpu, memory, disk] = [Metric::Cpu, Metric::Memory, Metric::Disk].map(|metric| {
            let i = metric.index();
            let mut m = Measurement::project(&self.totals[i], metric, at, running, self.unknown[i]);
            m.mean = self.means[i];
            m.max = self.peaks[i];
            m.latest = self.latest[i];
            m.last_observed = self.last_observed[i];
            // A sum of non-synchronous observations is no fresher than its
            // oldest contributor. Coverage/excluded_count remain visible.
            m.observed_at = self.oldest_sample[i];
            if m.latest.is_some() {
                m.state = if self.unknown[i] > 0. || m.coverage_ratio.is_none_or(|c| c < 0.999) {
                    MetricState::Partial
                } else {
                    MetricState::Ok
                };
            }
            if metric == Metric::Disk {
                m.filesystem_capacity_bytes = self.filesystem_capacity;
            }
            m
        });
        Measured { cpu, memory, disk }
    }
}
