//! Duration guards also account for cancelled operations, without dynamic outcome labels.
use super::names;
use std::time::Instant;

pub struct Operation {
    metric: &'static str,
    label: &'static str,
    value: String,
    outcome: &'static str,
    started: Instant,
}
impl Operation {
    pub fn new(metric: &'static str, label: &'static str, value: impl Into<String>) -> Self {
        Self {
            metric,
            label,
            value: value.into(),
            outcome: "cancelled",
            started: Instant::now(),
        }
    }
    pub fn finish(&mut self, outcome: &'static str) {
        self.outcome = outcome;
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        metrics::histogram!(self.metric, self.label => self.value.clone(), names::OUTCOME => self.outcome)
            .record(self.started.elapsed().as_secs_f64());
    }
}
pub fn provider_error(provider: &str, kind: &'static str) {
    metrics::counter!(names::PROVIDER_ERRORS, names::PROVIDER => provider.to_owned(), names::KIND => kind).increment(1);
}
