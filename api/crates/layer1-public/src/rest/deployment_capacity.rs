//! REST details for the deployment-wide admission refusal.

use axum::http::StatusCode;

/// This is service availability, not an allowance the caller exceeded.
///
/// A per-owner session limit is a conflict because that owner can release one
/// of their own slots. Deployment room may be held by somebody else entirely,
/// and no honest `Retry-After` exists because time is not what releases it.
pub(super) fn status_and_slug() -> (StatusCode, &'static str) {
    (StatusCode::SERVICE_UNAVAILABLE, "deployment-at-capacity")
}

/// Keep the four actionable figures machine-readable rather than buried in
/// the English detail sentence.
pub(super) fn insert_fields(
    object: &mut serde_json::Map<String, serde_json::Value>,
    requested_cpu_millis: u64,
    requested_memory_mib: u64,
    free_cpu_millis: u64,
    free_memory_mib: u64,
) {
    object.insert("requested_cpu_millis".into(), requested_cpu_millis.into());
    object.insert("requested_memory_mib".into(), requested_memory_mib.into());
    object.insert("free_cpu_millis".into(), free_cpu_millis.into());
    object.insert("free_memory_mib".into(), free_memory_mib.into());
}
