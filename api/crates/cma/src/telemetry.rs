//! Bounded process observations. Durable admission/terminal accounting belongs
//! to the Mongo ledger; these counters never assert exactly-once work effects.
pub(crate) mod browser;
pub(crate) mod http;
pub(crate) mod request;
pub(crate) mod server;
pub(crate) mod worker;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

pub(crate) fn install() -> Result<PrometheusHandle, String> {
    let recorder = PrometheusBuilder::new()
        .set_buckets(&[
            0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1., 2., 5., 10., 30., 120., 600., 1800.,
            3600.,
        ])
        .map_err(|error| error.to_string())?
        .build_recorder();
    let handle = recorder.handle();
    metrics::set_global_recorder(recorder)
        .map_err(|_| "a metrics recorder is already installed".to_owned())?;
    metrics::describe_gauge!(
        "cma_telemetry_sample_timestamp_seconds",
        "Last local diagnostics sample; not scrape time"
    );
    metrics::describe_gauge!(
        "cma_readiness_check_success",
        "Last completed Mongo readiness result; also check its timestamp"
    );
    metrics::describe_gauge!(
        "cma_readiness_check_timestamp_seconds",
        "Last completed Mongo readiness check"
    );
    metrics::gauge!("cma_readiness_check_success").set(0);
    metrics::gauge!("cma_readiness_check_timestamp_seconds").set(0);
    browser::initialize();
    worker::initialize();
    sample();
    Ok(handle)
}

pub(crate) fn timestamp() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub(crate) fn sample() {
    metrics::gauge!("cma_telemetry_sample_timestamp_seconds").set(timestamp());
}
