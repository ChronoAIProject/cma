use metrics::{Counter, Gauge, Histogram, Key, KeyName, Metadata, SharedString, Unit};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(super) struct Identity {
    pub resolutions: u64,
    pub resolution_us: u64,
    pub resolved: u64,
    pub refused: u64,
    pub cancelled: u64,
    pub provider_dispatches: u64,
    pub invalid: bool,
}

#[derive(Clone, Default)]
pub(super) struct Recorder(pub Arc<Mutex<Identity>>);

impl metrics::CounterFn for Recorder {
    fn increment(&self, value: u64) {
        let mut sample = self.0.lock().unwrap();
        sample.provider_dispatches = sample.provider_dispatches.checked_add(value).unwrap();
    }

    fn absolute(&self, _: u64) {
        self.0.lock().unwrap().invalid = true;
    }
}

struct Resolution {
    sample: Recorder,
    outcome: Option<&'static str>,
}

impl metrics::HistogramFn for Resolution {
    fn record(&self, value: f64) {
        let mut sample = self.sample.0.lock().unwrap();
        sample.resolutions += 1;
        let micros = value * 1_000_000.0;
        if !micros.is_finite() || micros < 0.0 || micros >= u64::MAX as f64 {
            sample.invalid = true;
            return;
        }
        sample.resolution_us = sample
            .resolution_us
            .checked_add(micros.round() as u64)
            .unwrap();
        match self.outcome {
            Some("resolved") => sample.resolved += 1,
            Some("unavailable" | "anonymous" | "rejected") => sample.refused += 1,
            Some("cancelled") => sample.cancelled += 1,
            _ => sample.invalid = true,
        }
    }
}

impl metrics::Recorder for Recorder {
    fn describe_counter(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
    fn describe_gauge(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
    fn describe_histogram(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}

    fn register_counter(&self, key: &Key, _: &Metadata<'_>) -> Counter {
        if key.name() == "cma_identity_provider_calls_total" {
            Counter::from_arc(Arc::new(self.clone()))
        } else {
            Counter::noop()
        }
    }

    fn register_gauge(&self, _: &Key, _: &Metadata<'_>) -> Gauge {
        Gauge::noop()
    }

    fn register_histogram(&self, key: &Key, _: &Metadata<'_>) -> Histogram {
        if key.name() != "cma_auth_resolution_seconds" {
            return Histogram::noop();
        }
        let ambient = key
            .labels()
            .any(|label| label.key() == "path" && label.value() == "ambient");
        let outcome = ambient
            .then(|| {
                key.labels().find_map(|label| {
                    if label.key() != "outcome" {
                        return None;
                    }
                    match label.value() {
                        "resolved" => Some("resolved"),
                        "unavailable" => Some("unavailable"),
                        "anonymous" => Some("anonymous"),
                        "rejected" => Some("rejected"),
                        "cancelled" => Some("cancelled"),
                        _ => None,
                    }
                })
            })
            .flatten();
        Histogram::from_arc(Arc::new(Resolution {
            sample: self.clone(),
            outcome,
        }))
    }
}
