use super::chart_projections::{append_line, ProjectionBatch};
use super::charts::{merge_buckets, metric_expectations};
use super::*;
use layer2_core::sandbox_metrics::{allocation::Expectation, series, Metric, Report};
use std::collections::BTreeMap;

/// Accumulate the report's complete totals in the same Sandbox order as the
/// frozen chart reader. The worker owns this state for exactly one generation.
pub struct TotalProjection {
    report_id: String,
    generation: i64,
    holder: String,
    committed_members: i64,
    last_sandbox: Option<String>,
    members: i64,
    totals: [Option<Vec<series::Bucket>>; 3],
    scratch: Vec<series::Bucket>,
}

impl TotalProjection {
    pub fn new(report_id: &str, generation: i64, holder: &str) -> Self {
        Self {
            report_id: report_id.into(),
            generation,
            holder: holder.into(),
            committed_members: 0,
            last_sandbox: None,
            members: 0,
            totals: Default::default(),
            scratch: Vec::new(),
        }
    }

    pub fn acknowledge_source(&mut self, count: usize) -> Result<()> {
        self.committed_members += i64::try_from(count).map_err(unavailable)?;
        if self.committed_members != self.members {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }

    pub fn add_member(
        &mut self,
        report: &Report,
        sandbox: &str,
        buckets: &[BTreeMap<u64, series::Bucket>; 3],
        expectations: &[Expectation],
    ) -> Result<()> {
        if self.report_id != report.report_id
            || self
                .last_sandbox
                .as_ref()
                .is_some_and(|last| last.as_str() >= sandbox)
        {
            return Err(MetricsError::Unavailable);
        }
        self.last_sandbox = Some(sandbox.into());
        self.members += 1;
        for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
            series::complete_into(
                &mut self.scratch,
                &buckets[metric.index()],
                &metric_expectations(expectations, metric),
                metric,
                report.effective_range,
                report.query.step_seconds,
            );
            merge_buckets(
                self.totals[metric.index()].get_or_insert_with(Vec::new),
                &self.scratch,
            );
        }
        Ok(())
    }

    pub fn append(
        self,
        base: &Document,
        batch: &mut ProjectionBatch,
        expected_members: i64,
        holder: &str,
    ) -> Result<()> {
        if self.members != expected_members
            || self.committed_members != expected_members
            || self.report_id != base.get_str("report_id").map_err(unavailable)?
            || self.generation != base.get_i64("generation").map_err(unavailable)?
            || self.holder != holder
        {
            return Err(MetricsError::Unavailable);
        }
        for (metric, points) in [Metric::Cpu, Metric::Memory, Metric::Disk]
            .into_iter()
            .zip(self.totals)
        {
            let mut base = base.clone();
            base.insert("metric", metric.index() as i64);
            append_line(batch, &base, "chart_total", "", points)?;
        }
        Ok(())
    }
}
