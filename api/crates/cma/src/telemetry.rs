//! Bounded process observations. Durable admission/terminal accounting belongs
//! to the Mongo ledger; these counters never assert exactly-once work effects.
pub(crate) mod agent_output;
pub(crate) mod browser;
pub(crate) mod command_reconcile;
pub(crate) mod http;
pub(crate) mod request;
pub(crate) mod runtime;
pub(crate) mod server;
pub(crate) mod task_cohort;
pub(crate) mod work;
pub(crate) mod worker;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

pub(crate) fn install() -> Result<PrometheusHandle, String> {
    let builder = PrometheusBuilder::new()
        .set_buckets(&[
            0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1., 2., 5., 10., 30., 120., 600., 1800.,
            3600.,
        ])
        .map_err(|error| error.to_string())?;
    let builder = configure_histograms(builder)?;
    let recorder = builder.build_recorder();
    let handle = recorder.handle();
    metrics::set_global_recorder(recorder)
        .map_err(|_| "a metrics recorder is already installed".to_owned())?;
    metrics::describe_gauge!(
        "cma_telemetry_sample_timestamp_seconds",
        "Last local diagnostics sample; not scrape time"
    );
    metrics::describe_counter!(
        "cma_document_storage_receipt_mismatch_total",
        "Document storage receipt integrity mismatches by field"
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
    work::initialize();
    browser::initialize();
    worker::initialize();
    command_reconcile::initialize();
    runtime::initialize();
    metrics::describe_histogram!(
        "cma_agent_log_scan_longest_section_seconds",
        "Longest uninterrupted execution section per complete scan attempt"
    );
    metrics::describe_histogram!(
        "cma_agent_log_scan_driver_poll_seconds",
        "Longest final ready cursor poll per complete scan attempt"
    );
    metrics::describe_counter!(
        "cma_agent_log_scan_yields_total",
        "Explicit cooperative yields during complete scan attempts"
    );
    sample();
    Ok(handle)
}

fn configure_histograms(builder: PrometheusBuilder) -> Result<PrometheusBuilder, String> {
    let mut builder = runtime::configure(builder)?;
    builder = builder
        .set_buckets_for_metric(
            metrics_exporter_prometheus::Matcher::Full("cma_agent_output_mongo_commands".into()),
            &[0., 1., 2., 4., 6., 8., 16., 32., 64., 128., 256.],
        )
        .map_err(|error| error.to_string())?;
    // These thresholds distinguish #1787's sub-2 ms, 5 ms driver and 10 ms
    // section cases; the general request histogram starts too coarsely.
    for name in [
        "cma_agent_log_scan_longest_section_seconds",
        "cma_agent_log_scan_driver_poll_seconds",
    ] {
        builder = builder
            .set_buckets_for_metric(
                metrics_exporter_prometheus::Matcher::Full(name.into()),
                &[
                    0.00025, 0.0005, 0.001, 0.002, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.,
                    2., 5., 10.,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(builder)
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
