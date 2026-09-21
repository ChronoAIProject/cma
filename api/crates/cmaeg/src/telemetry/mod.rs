//! Process metrics registration; the HTTP listener lives on a separate address.
pub mod http;
pub mod names;
pub mod operations;
pub mod timing;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
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
            Ok(handle)
        })
        .clone()
}
