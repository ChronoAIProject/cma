//! A whole cohort result is replaced/rendered under one lock. Publishing many
//! independent gauges can let a scrape mix two batches even on one replica.
use std::{
    collections::BTreeMap,
    fmt::Write,
    sync::{Arc, RwLock},
};

#[derive(Clone, Default)]
pub(crate) struct Cache(Arc<RwLock<BTreeMap<&'static str, Sample>>>);

#[derive(Default)]
struct Sample {
    attempted: f64,
    success: bool,
    error: &'static str,
    last_good: Option<(f64, String)>,
}

impl Cache {
    pub(crate) fn initialize(&self, windows: &[&'static str]) {
        let mut samples = self.0.write().expect("cohort cache lock");
        for window in windows {
            samples.entry(window).or_default();
        }
    }

    pub(crate) fn update(&self, window: &'static str, result: Result<String, &'static str>) {
        let now = super::timestamp();
        let mut samples = self.0.write().expect("cohort cache lock");
        let sample = samples.entry(window).or_default();
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
        let samples = self.0.read().expect("cohort cache lock");
        let mut text = String::new();
        for (metric, help) in [
            ("tasks", "Message command receipt states; public/native terminal evidence is separate"),
            ("accepted_tasks", "Independent count of message receipts in the named admission interval"),
            ("control_commands", "Classified non-message command receipts; not tasks"),
            ("classification_complete", "One only when every selected receipt metadata row classified; not end-to-end proof"),
            ("diagnostic_tasks", "Overlapping persisted receipt diagnostics, never additional lifecycle states"),
            ("unfinished_age_items", "Disjoint admission-age bands for unfinished message receipts; snapshot gauges"),
            ("evidence_supported", "One when this observer can verify the evidence source; zero means unsupported, not healthy"),
        ] {
            writeln!(text, "# HELP cma_task_cohort_{metric} {help}\n# TYPE cma_task_cohort_{metric} gauge").unwrap();
        }
        for (window, sample) in samples.iter() {
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
        }
        text
    }
}

#[cfg(test)]
mod tests;
