//! Process metrics registration; the HTTP listener lives on a separate address.
pub(crate) mod delivery;
pub mod http;
pub(crate) mod http_failure;
pub mod names;
pub mod operations;
pub mod provider_start;
pub mod timing;
pub(crate) mod turn_trace;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;
static REGISTRY: OnceLock<Result<PrometheusHandle, String>> = OnceLock::new();
pub use cmaeg_execution::channel_hash;
pub fn install() -> Result<PrometheusHandle, String> {
    REGISTRY
        .get_or_init(|| {
            let recorder = PrometheusBuilder::new()
                .set_buckets(&names::BUCKETS)
                .map_err(|error| error.to_string())?
                .set_buckets_for_metric(
                    Matcher::Full(names::TURN_DURATION.into()),
                    &[0.1, 0.5, 1., 2., 5., 10., 30., 120., 600., 1800., 3600.],
                )
                .map_err(|error| error.to_string())?
                .set_buckets_for_metric(
                    Matcher::Prefix("cmaeg_delivery_queue_".into()),
                    &[0.005, 0.025, 0.1, 0.5, 1., 5., 10., 30., 120., 600.],
                )
                .map_err(|error| error.to_string())?
                .set_buckets_for_metric(
                    Matcher::Full("cmaeg_provider_permit_held_seconds".into()),
                    &[
                        0.005, 0.025, 0.1, 0.5, 1., 5., 30., 120., 600., 1800., 3600.,
                    ],
                )
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
            metrics::describe_counter!(
                "cmaeg_delivery_outcomes_total",
                "Logical delivery step results; local buffering is not external confirmation"
            );
            for stage in ["inbound", "worker"] {
                for name in [
                    "cmaeg_delivery_queue_depth",
                    "cmaeg_delivery_queue_capacity",
                    "cmaeg_delivery_queue_waiters",
                    "cmaeg_delivery_queue_sample_timestamp_seconds",
                ] {
                    metrics::gauge!(name, "stage" => stage).set(0.);
                }
            }
            metrics::gauge!("cmaeg_delivery_queue_oldest_seconds", "stage" => "worker").set(0.);
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
    metrics::describe_histogram!(
        names::FENCE_WAIT,
        metrics::Unit::Seconds,
        "Time a turn waited for its conversation predecessor to start"
    );
    let _ = metrics::histogram!(names::FENCE_WAIT);
    metrics::describe_histogram!(
        names::MEDIA_PREPARE,
        metrics::Unit::Seconds,
        "Time a turn spent on its eager attachment items, by outcome"
    );
    let _ = metrics::histogram!(names::MEDIA_PREPARE);
    metrics::describe_histogram!(
        names::MEDIA_TRANSFORM_SECONDS,
        metrics::Unit::Seconds,
        "Sandboxed media job time, spawn to reap, by op"
    );
    let _ = metrics::histogram!(names::MEDIA_TRANSFORM_SECONDS);
    metrics::describe_counter!(
        names::MEDIA_TRANSFORM_TOTAL,
        "Sandboxed media jobs by op and outcome"
    );
    metrics::counter!(names::MEDIA_TRANSFORM_TOTAL).absolute(0);
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
    metrics::describe_counter!(
        names::MEDIA_DOWNLOADS,
        metrics::Unit::Count,
        "CMAEG media link downloads by outcome"
    );
    metrics::counter!(names::MEDIA_DOWNLOADS).absolute(0);
    metrics::describe_counter!(
        names::MEDIA_LINKS,
        metrics::Unit::Count,
        "CMAEG media download link mints by outcome"
    );
    metrics::counter!(names::MEDIA_LINKS).absolute(0);
    metrics::describe_counter!(
        names::STT_AUDIO_SECONDS,
        metrics::Unit::Seconds,
        "CMAEG metered speech-to-text audio seconds, reconciled with reported usage"
    );
    metrics::counter!(names::STT_AUDIO_SECONDS).absolute(0);
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
        (
            names::STT_CHANNELS_AT_BUDGET,
            metrics::Unit::Count,
            "CMAEG channels refused by their own speech-to-text budget in the last hour",
        ),
        (
            names::MEDIA_SANDBOX_READY,
            metrics::Unit::Count,
            "1 once the media transformer's sandbox self-check passed at startup",
        ),
    ] {
        metrics::describe_gauge!(name, unit, description);
        metrics::gauge!(name).set(0);
    }
}

/// Console metrics, registered at zero only when the console is built, so a
/// gateway with the console off exports none of them.
pub(crate) fn describe_console() {
    metrics::describe_gauge!(
        names::CONSOLE_ENABLED,
        metrics::Unit::Count,
        "1 while the console's OAuth client is configured and NyxID discovery has succeeded"
    );
    metrics::gauge!(names::CONSOLE_ENABLED).set(0);
    metrics::describe_gauge!(
        names::CONSOLE_SESSIONS,
        metrics::Unit::Count,
        "Console sessions held in memory"
    );
    metrics::gauge!(names::CONSOLE_SESSIONS).set(0);
    metrics::describe_counter!(
        names::CONSOLE_SESSION_EVICTIONS,
        "Console sessions ended by a cap or an expiry, by reason"
    );
    metrics::counter!(names::CONSOLE_SESSION_EVICTIONS).absolute(0);
    metrics::describe_counter!(
        names::CONSOLE_REVOCATIONS,
        "Best-effort refresh token revocations of ended console sessions, by outcome"
    );
    metrics::counter!(names::CONSOLE_REVOCATIONS).absolute(0);
    metrics::describe_counter!(
        names::CONSOLE_REJECTIONS,
        "Console form posts refused for a foreign origin or a bad CSRF token, by reason"
    );
    metrics::counter!(names::CONSOLE_REJECTIONS).absolute(0);
    metrics::describe_counter!(names::CONSOLE_LOGINS, "Console sign-in steps, by outcome");
    metrics::counter!(names::CONSOLE_LOGINS).absolute(0);
    metrics::describe_counter!(
        names::CONSOLE_AUTHORIZATIONS,
        "Console requests whose owner authority was derived, by outcome"
    );
    metrics::counter!(names::CONSOLE_AUTHORIZATIONS).absolute(0);
    metrics::describe_counter!(
        names::CONSOLE_OBSERVER_DROPPED,
        "Console observations dropped because the observation queue was full"
    );
    metrics::counter!(names::CONSOLE_OBSERVER_DROPPED).absolute(0);
    metrics::describe_counter!(
        names::CONSOLE_UNAUTHENTICATED,
        "Ingress requests that failed authentication, by source; never recorded in console memory"
    );
    metrics::counter!(names::CONSOLE_UNAUTHENTICATED).absolute(0);
    metrics::describe_gauge!(
        names::CONSOLE_INDEX_RECORDS,
        metrics::Unit::Count,
        "Channel record heads in the console's owner index"
    );
    metrics::gauge!(names::CONSOLE_INDEX_RECORDS).set(0);
    metrics::describe_gauge!(
        names::CONSOLE_INDEX_PARTIAL,
        metrics::Unit::Count,
        "1 while the console's owner index is at its bound and may miss channels"
    );
    metrics::gauge!(names::CONSOLE_INDEX_PARTIAL).set(0);
    metrics::describe_gauge!(
        names::CONSOLE_ACTIVITY_BYTES,
        metrics::Unit::Bytes,
        "Bytes counted by the console's activity entries and producer entries"
    );
    metrics::gauge!(names::CONSOLE_ACTIVITY_BYTES).set(0);
    metrics::describe_counter!(
        names::CONSOLE_REQUESTS,
        "Console requests by route template and outcome"
    );
    metrics::counter!(names::CONSOLE_REQUESTS).absolute(0);
}
