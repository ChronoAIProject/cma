//! Process metrics registration; the HTTP listener lives on a separate address.
pub mod http;
pub mod names;
pub mod operations;
pub mod timing;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;
static REGISTRY: OnceLock<Result<PrometheusHandle, String>> = OnceLock::new();
pub fn channel_hash(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(id.as_bytes());
    hash[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}
pub fn install() -> Result<PrometheusHandle, String> {
    REGISTRY
        .get_or_init(|| {
            let recorder = PrometheusBuilder::new()
                .set_buckets(&names::BUCKETS)
                .map_err(|error| error.to_string())?
                .set_buckets_for_metric(
                    Matcher::Prefix("cmaeg_media_".into()),
                    &names::MEDIA_BUCKETS,
                )
                .map_err(|error| error.to_string())?
                .build_recorder();
            let handle = recorder.handle();
            metrics::set_global_recorder(recorder)
                .map_err(|_| "a metrics recorder is already installed".to_owned())?;
            for name in names::HISTOGRAMS {
                metrics::describe_histogram!(
                    name,
                    metrics::Unit::Seconds,
                    "CMAEG operation duration in seconds"
                );
                let _ = metrics::histogram!(name);
            }
            for name in names::COUNTERS {
                metrics::describe_counter!(name, "CMAEG classified operation count");
                metrics::counter!(name).absolute(0);
            }
            for name in names::GAUGES {
                metrics::describe_gauge!(name, "CMAEG current local object count");
                metrics::gauge!(name).set(0);
            }
            describe_media();
            Ok(handle)
        })
        .clone()
}

/// Media metrics are registered at zero like the rest, with their own units.
fn describe_media() {
    metrics::describe_histogram!(
        names::MEDIA_FETCH_SECONDS,
        metrics::Unit::Seconds,
        "CMAEG media fetch duration in seconds by source and outcome"
    );
    let _ = metrics::histogram!(names::MEDIA_FETCH_SECONDS);
    metrics::describe_histogram!(
        names::MEDIA_DERIVE,
        metrics::Unit::Seconds,
        "Media derivation time by artifact and outcome"
    );
    let _ = metrics::histogram!(names::MEDIA_DERIVE);
    metrics::describe_counter!(
        names::MEDIA_FETCH_BYTES,
        metrics::Unit::Bytes,
        "CMAEG media bytes fetched by source"
    );
    metrics::counter!(names::MEDIA_FETCH_BYTES).absolute(0);
    metrics::describe_counter!(
        names::MEDIA_TOOL_READS,
        metrics::Unit::Count,
        "CMAEG attachment reads by requested view and resulting status"
    );
    metrics::counter!(names::MEDIA_TOOL_READS).absolute(0);
    for (name, unit, description) in [
        (
            names::MEDIA_INFLIGHT_BYTES,
            metrics::Unit::Bytes,
            "CMAEG bytes reserved by fetched media originals and transform outputs",
        ),
        (
            names::MEDIA_RESPONSE_BYTES,
            metrics::Unit::Bytes,
            "CMAEG weighted bytes reserved by media response bodies in flight",
        ),
        (
            names::MEDIA_JOBS_INFLIGHT,
            metrics::Unit::Count,
            "CMAEG media jobs holding a pod-wide job permit",
        ),
        (
            names::MEDIA_CACHE_BYTES,
            metrics::Unit::Bytes,
            "CMAEG weighted bytes held by the derived-media cache",
        ),
    ] {
        metrics::describe_gauge!(name, unit, description);
        metrics::gauge!(name).set(0);
    }
}
