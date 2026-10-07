//! Stable metric and label names. Call sites use bounded protocol classifications.
pub const INGRESS_TO_PROVIDER: &str = "cmaeg_ingress_to_provider_seconds";
pub const RESTORE_DURATION: &str = "cmaeg_restore_duration_seconds";
pub const PROVIDER_ACCEPT: &str = "cmaeg_provider_accept_seconds";
pub const TURN_DURATION: &str = "cmaeg_turn_duration_seconds";
pub const DELIVERY_DURATION: &str = "cmaeg_delivery_seconds";
pub const TOOL_DURATION: &str = "cmaeg_tool_call_seconds";
pub const EVENTS: &str = "cmaeg_events_total";
pub const REFUSALS: &str = "cmaeg_refusals_total";
pub const PROVIDER_ERRORS: &str = "cmaeg_provider_errors_total";
pub const TURNS_INFLIGHT: &str = "cmaeg_turns_inflight";
pub const CHANNELS_CACHED: &str = "cmaeg_channels_cached";
pub const RELAY_ENVELOPE_REPAIRS: &str = "cmaeg_relay_envelope_repairs_total";
pub const CALLBACK_INVALID: &str = "cmaeg_callback_invalid_total";
pub const DELIVERY_WORKER_RESTARTS: &str = "cmaeg_delivery_worker_restarts_total";
pub const MEDIA_ACTIVITY_DEGRADED: &str = "cmaeg_media_activity_degraded_total";
pub const SOURCE: &str = "source";
pub const COLD: &str = "cold";
pub const OUTCOME: &str = "outcome";
pub const PROVIDER: &str = "provider";
pub const PLATFORM: &str = "platform";
pub const TOOL: &str = "tool";
pub const REASON: &str = "reason";
pub const KIND: &str = "kind";
pub const FIELD: &str = "field";
pub const DETAIL: &str = "detail";
pub const HISTOGRAMS: [&str; 6] = [
    INGRESS_TO_PROVIDER,
    RESTORE_DURATION,
    PROVIDER_ACCEPT,
    TURN_DURATION,
    DELIVERY_DURATION,
    TOOL_DURATION,
];
pub const COUNTERS: [&str; 7] = [
    EVENTS,
    REFUSALS,
    PROVIDER_ERRORS,
    RELAY_ENVELOPE_REPAIRS,
    CALLBACK_INVALID,
    DELIVERY_WORKER_RESTARTS,
    MEDIA_ACTIVITY_DEGRADED,
];
pub const GAUGES: [&str; 2] = [TURNS_INFLIGHT, CHANNELS_CACHED];
pub const BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0,
];
