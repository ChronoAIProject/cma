use super::measured_model::{FrozenBinding, FrozenSeries};
use super::{FrozenMeasuredMinute, MeasuredError};
use crate::sandbox_metrics::{aggregate::aggregate_observing, Metric, Range};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct ObservedSupport {
    pub binding_id: String,
    pub metric: Metric,
    pub intervals: Vec<(f64, f64)>,
}

impl FrozenMeasuredMinute {
    /// Reuse the integrator's accepted segments, before duration quantization.
    pub fn observed_support(&self) -> Result<Vec<ObservedSupport>, MeasuredError> {
        if self.version != 1 || self.algorithm != "binding-integral-half-even-v1" {
            return Err(MeasuredError::Invalid);
        }
        if !self.source_complete {
            return Ok(vec![]);
        }
        let bindings = self
            .bindings
            .iter()
            .map(FrozenBinding::restore)
            .collect::<Result<Vec<_>, _>>()?;
        let bindings: Vec<_> = bindings
            .into_iter()
            .filter(|b| {
                b.sandbox_id == self.expected.sandbox_id && b.owner_subject == self.expected.owner
            })
            .collect();
        let series: Vec<_> = self.series.iter().map(FrozenSeries::restore).collect();
        let mut support: BTreeMap<(String, Metric), Vec<(f64, f64)>> = BTreeMap::new();
        aggregate_observing(
            &bindings,
            &series,
            Range {
                from: self.expected.bucket_start,
                to: self.expected.bucket_start + 60,
            },
            &mut |binding, metric, from, to| {
                let intervals = support.entry((binding.id.clone(), metric)).or_default();
                if let Some(last) = intervals.last_mut().filter(|last| from <= last.1) {
                    last.1 = last.1.max(to);
                } else {
                    intervals.push((from, to));
                }
            },
        )
        .map_err(|_| MeasuredError::Attribution)?;
        Ok(support
            .into_iter()
            .map(|((binding_id, metric), intervals)| ObservedSupport {
                binding_id,
                metric,
                intervals,
            })
            .collect())
    }
}
