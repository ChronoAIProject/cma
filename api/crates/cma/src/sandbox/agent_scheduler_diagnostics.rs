//! Emit only the bounded projection, isolated from private caller span fields.
use super::super::lifecycle_failure::FailureObservation;

pub(super) fn failed(value: &FailureObservation) {
    // JSON span lists use the current span even for parent:none events. A
    // field-free root is required for both the text and JSON dispatchers.
    let root =
        tracing::error_span!(target: "agent_scheduler_failure", parent: None, "agent_scheduler");
    // A selective EnvFilter can disable the root while enabling the event.
    // In that case suppress the diagnostic, never inherit private ancestry.
    if root.is_disabled() {
        return;
    }
    let _entered = root.enter();
    let phase = value.phase.as_str();
    let origin = value.origin.as_str();
    let cause_class = value.cause_class.as_str();
    let detail_code = value.detail_code.as_str();
    let agent_hash = value.agent_hash.as_deref();
    tracing::error!(target: "agent_scheduler_failure", parent: None,
        phase,
        origin,
        cause_class,
        detail_code,
        detail_retained = value.detail_retained,
        detail_suppressed = value.detail_suppressed,
        attempt_id = %value.attempt_id,
        agent_hash,
        elapsed_ms = value.elapsed_ms,
        continuation_index = value.continuation_index,
        "agent lifecycle task failed"
    );
}
