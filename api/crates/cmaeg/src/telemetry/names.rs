//! Stable metric and label names. Call sites use bounded protocol classifications.
pub const INGRESS_TO_PROVIDER: &str = "cmaeg_ingress_to_provider_seconds";
pub const RESTORE_DURATION: &str = "cmaeg_restore_duration_seconds";
pub const PROVIDER_ACCEPT: &str = "cmaeg_provider_accept_seconds";
pub const TURN_DURATION: &str = "cmaeg_turn_duration_seconds";
pub const DELIVERY_DURATION: &str = "cmaeg_delivery_seconds";
pub const TOOL_DURATION: &str = "cmaeg_tool_call_seconds";
pub const TOOL_SENDS_LATE: &str = "cmaeg_tool_sends_late_total";
pub const EVENTS: &str = "cmaeg_events_total";
pub const REFUSALS: &str = "cmaeg_refusals_total";
pub const PROVIDER_ERRORS: &str = "cmaeg_provider_errors_total";
pub const TURNS_INFLIGHT: &str = "cmaeg_turns_inflight";
pub const CHANNELS_CACHED: &str = "cmaeg_channels_cached";
pub const RELAY_ENVELOPE_REPAIRS: &str = "cmaeg_relay_envelope_repairs_total";
pub const CALLBACK_INVALID: &str = "cmaeg_callback_invalid_total";
pub const DELIVERY_WORKER_RESTARTS: &str = "cmaeg_delivery_worker_restarts_total";
pub const DELIVERY_LABELS: &str = "cmaeg_delivery_labels_total";
pub const DEFAULT_REPLY: &str = "cmaeg_default_reply_total";
pub const ROUTING_NOTICES_DROPPED: &str = "cmaeg_routing_notices_dropped_total";
pub const MEDIA_ACTIVITY_DEGRADED: &str = "cmaeg_media_activity_degraded_total";
pub const MEDIA_FACTS_REPAIRED: &str = "cmaeg_media_facts_repaired_total";
pub const MEDIA_ITEMS: &str = "cmaeg_media_items_total";
pub const MEDIA_RELAY_CORRELATION: &str = "cmaeg_media_relay_correlation_total";
pub const MEDIA_FETCH_SECONDS: &str = "cmaeg_media_fetch_seconds";
pub const MEDIA_FETCH_BYTES: &str = "cmaeg_media_fetch_bytes_total";
pub const MEDIA_INFLIGHT_BYTES: &str = "cmaeg_media_inflight_bytes";
pub const MEDIA_RESPONSE_BYTES: &str = "cmaeg_media_response_bytes";
pub const MEDIA_JOBS_INFLIGHT: &str = "cmaeg_media_jobs_inflight";
pub const MEDIA_CACHE_BYTES: &str = "cmaeg_media_cache_bytes";
pub const MEDIA_DERIVE: &str = "cmaeg_media_derive_seconds";
pub const MEDIA_TOOL_READS: &str = "cmaeg_media_tool_reads_total";
pub const MEDIA_DOWNLOADS: &str = "cmaeg_media_downloads_total";
pub const MEDIA_LINKS: &str = "cmaeg_media_links_total";
pub const STT_AUDIO_SECONDS: &str = "cmaeg_stt_audio_seconds_total";
pub const STT_CHANNELS_AT_BUDGET: &str = "cmaeg_stt_channels_at_budget";
pub const FENCE_WAIT: &str = "cmaeg_media_fence_wait_seconds";
pub const MEDIA_PREPARE: &str = "cmaeg_media_prepare_seconds";
pub const MEDIA_TRANSFORM_TOTAL: &str = "cmaeg_media_transform_total";
pub const MEDIA_TRANSFORM_SECONDS: &str = "cmaeg_media_transform_seconds";
pub const MEDIA_SANDBOX_READY: &str = "cmaeg_media_sandbox_ready";
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
pub const OP: &str = "op";
pub const VIEW: &str = "view";
pub const STATUS: &str = "status";
pub const MEDIA: &str = "media";
pub const TRANSPORT: &str = "transport";
pub const HISTOGRAMS: [&str; 6] = [
    INGRESS_TO_PROVIDER,
    RESTORE_DURATION,
    PROVIDER_ACCEPT,
    TURN_DURATION,
    DELIVERY_DURATION,
    TOOL_DURATION,
];
pub const FANOUT_FAILED_DROPPED: &str = "cmaeg_fanout_consumer_failed_dropped_total";
pub const FANOUT_TURNS: &str = "cmaeg_fanout_turns_total";
pub const COUNTERS: [&str; 16] = [
    TOOL_SENDS_LATE,
    FANOUT_FAILED_DROPPED,
    FANOUT_TURNS,
    EVENTS,
    REFUSALS,
    PROVIDER_ERRORS,
    RELAY_ENVELOPE_REPAIRS,
    CALLBACK_INVALID,
    DELIVERY_WORKER_RESTARTS,
    DELIVERY_LABELS,
    DEFAULT_REPLY,
    ROUTING_NOTICES_DROPPED,
    MEDIA_ACTIVITY_DEGRADED,
    MEDIA_FACTS_REPAIRED,
    MEDIA_ITEMS,
    MEDIA_RELAY_CORRELATION,
];
pub const ROUTED_TURNS_IN_FLIGHT: &str = "cmaeg_routed_turns_in_flight";
pub const GAUGES: [&str; 3] = [TURNS_INFLIGHT, CHANNELS_CACHED, ROUTED_TURNS_IN_FLIGHT];
pub const BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0,
];
/// Media metrics carry their own units and descriptions, so they are listed
/// apart from the generic families above.
pub const MEDIA_HISTOGRAMS: [&str; 5] = [
    MEDIA_FETCH_SECONDS,
    MEDIA_DERIVE,
    FENCE_WAIT,
    MEDIA_PREPARE,
    MEDIA_TRANSFORM_SECONDS,
];
pub const MEDIA_COUNTERS: [&str; 6] = [
    MEDIA_FETCH_BYTES,
    MEDIA_TOOL_READS,
    MEDIA_DOWNLOADS,
    MEDIA_LINKS,
    STT_AUDIO_SECONDS,
    MEDIA_TRANSFORM_TOTAL,
];
pub const MEDIA_GAUGES: [&str; 6] = [
    MEDIA_INFLIGHT_BYTES,
    MEDIA_RESPONSE_BYTES,
    MEDIA_JOBS_INFLIGHT,
    MEDIA_CACHE_BYTES,
    STT_CHANNELS_AT_BUDGET,
    MEDIA_SANDBOX_READY,
];
/// Media work spans an 8 s tool budget, a 15 s preparation budget and a 90 s
/// background budget, well past the 10 s top of [`BUCKETS`].
pub const MEDIA_BUCKETS: [f64; 13] = [
    0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 15.0, 30.0, 60.0, 90.0,
];
