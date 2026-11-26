//! A whole cohort result is replaced/rendered under one lock. Publishing many
//! independent gauges can let a scrape mix two batches even on one replica.
use std::{
    fmt::Write,
    sync::{Arc, RwLock},
};

#[derive(Clone, Default)]
pub(crate) struct Cache(Arc<RwLock<Option<Sample>>>);

#[derive(Default)]
struct Sample {
    attempted: f64,
    success: bool,
    error: &'static str,
    last_good: Option<(f64, String)>,
}

impl Cache {
    pub(crate) fn initialize(&self) {
        *self.0.write().expect("cohort cache lock") = Some(Sample::default());
    }

    pub(crate) fn update(&self, result: Result<String, &'static str>) {
        let now = super::timestamp();
        let mut sample = self.0.write().expect("cohort cache lock");
        let sample = sample.get_or_insert_with(Sample::default);
        sample.attempted = now;
        sample.success = result.is_ok();
        match result {
            Ok(text) => {
                sample.last_good = Some((now, text));
                sample.error = "none";
            }
            Err(error) => sample.error = error,
        }
    }

    pub(crate) fn render(&self) -> String {
        let sample = self.0.read().expect("cohort cache lock");
        let Some(sample) = sample.as_ref() else {
            return String::new();
        };
        let mut text = String::new();
        for (metric, help) in [
            (
                "tasks",
                "Message command receipt states; public/native terminal evidence is separate",
            ),
            (
                "accepted_tasks",
                "Independent count of message receipts in the named admission interval",
            ),
            (
                "classification_complete",
                "One when every selected message has a recognized terminal or unfinished outcome",
            ),
            (
                "audit_tasks",
                "Disjoint message classifications from durable terminal evidence",
            ),
            (
                "overdue_tasks",
                "Messages without a verified terminal beyond the review threshold",
            ),
        ] {
            writeln!(
                text,
                "# HELP cma_task_cohort_{metric} {help}\n# TYPE cma_task_cohort_{metric} gauge"
            )
            .unwrap();
        }
        let window = "release";
        for (name, value) in [
            ("sample_success", f64::from(sample.success)),
            ("sample_timestamp_seconds", sample.attempted),
            (
                "sample_success_timestamp_seconds",
                sample.last_good.as_ref().map_or(0., |(at, _)| *at),
            ),
        ] {
            writeln!(
                text,
                "cma_task_cohort_{name}{{window=\"{window}\"}} {value}"
            )
            .unwrap();
        }
        // Fixed vocabulary only. Do not expose driver errors, IDs or data.
        for reason in [
            "database",
            "budget",
            "invalid_snapshot",
            "timeout",
            "invalid_selection",
            "not_started",
        ] {
            writeln!(
                text,
                "cma_task_cohort_sample_error{{window=\"{window}\",reason=\"{reason}\"}} {}",
                u8::from(sample.error == reason)
            )
            .unwrap();
        }
        if let Some((_, result)) = &sample.last_good {
            text.push_str(result);
        }
        text
    }
}

#[cfg(test)]
mod tests;
