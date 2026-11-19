//! Fixed test diagnostic labels never expose store error payloads.
use layer2_core::sandbox::store::SandboxStoreError;

pub(in crate::store::sandbox_maintenance::mongo_integration_tests) fn classify_error(
    error: &SandboxStoreError,
) -> &'static str {
    match error {
        SandboxStoreError::Unavailable(message) => {
            if message.starts_with("sandbox maintenance exhausted its retry budget: ") {
                "retry_budget"
            } else if message.starts_with("sandbox maintenance commit remained unknown: ")
                || message.starts_with(
                    "sandbox maintenance commit remained unknown without an operation time: ",
                )
            {
                "unknown_commit"
            } else if message.starts_with(
                "sandbox maintenance commit exceeded its 30 second deadline; ambiguous commit: ",
            ) {
                "reconciliation_deadline"
            } else {
                "unavailable_other"
            }
        }
        SandboxStoreError::Budget(_) => "budget",
        SandboxStoreError::AdmissionPaused => "admission_paused",
        SandboxStoreError::SessionCapacityUnavailable => "session_capacity_unavailable",
        SandboxStoreError::NotFound => "not_found",
        SandboxStoreError::Duplicate => "duplicate",
        SandboxStoreError::Conflict => "conflict",
        SandboxStoreError::CheckpointInProgress => "checkpoint_in_progress",
        SandboxStoreError::LifecycleBusy { .. } => "lifecycle_busy",
        SandboxStoreError::SequenceConflict => "sequence_conflict",
        SandboxStoreError::AgentLogCapacityExceeded => "agent_log_capacity_exceeded",
        SandboxStoreError::LimitReached { .. } => "limit_reached",
        SandboxStoreError::GlobalCapacityReached => "global_capacity_reached",
        SandboxStoreError::SandboxNotReady => "sandbox_not_ready",
        SandboxStoreError::WorkspaceNameConflict => "workspace_name_conflict",
        SandboxStoreError::WorkspaceLimitReached { .. } => "workspace_limit_reached",
        SandboxStoreError::WorkspaceHasLiveAgents => "workspace_has_live_agents",
        SandboxStoreError::WorkspaceOriginConflict => "workspace_origin_conflict",
        SandboxStoreError::WorkspaceRecoveryConflict => "workspace_recovery_conflict",
        SandboxStoreError::SandboxOriginConflict => "sandbox_origin_conflict",
        SandboxStoreError::WorkspaceNotReady => "workspace_not_ready",
        SandboxStoreError::AgentLimitReached { .. } => "agent_limit_reached",
        SandboxStoreError::AgentCreationConflict => "agent_creation_conflict",
        SandboxStoreError::InvalidPage => "invalid_page",
        SandboxStoreError::InvalidTransaction(_) => "invalid_transaction",
        SandboxStoreError::Corrupt(_) => "corrupt",
    }
}

#[test]
fn maintenance_error_classification_recognizes_exact_origin_prefixes() {
    for (message, expected) in [
        (
            "sandbox maintenance exhausted its retry budget: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
            "retry_budget",
        ),
        (
            "sandbox maintenance commit remained unknown: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
            "unknown_commit",
        ),
        (
            "sandbox maintenance commit remained unknown without an operation time: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
            "unknown_commit",
        ),
        (
            "sandbox maintenance commit exceeded its 30 second deadline; ambiguous commit: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
            "reconciliation_deadline",
        ),
    ] {
        assert_eq!(
            classify_error(&SandboxStoreError::Unavailable(message.into())),
            expected
        );
    }
}

#[test]
fn maintenance_error_classification_rejects_nested_and_similar_prefixes() {
    for message in [
        "SYNTHETIC_DIAGNOSTIC_PAYLOAD",
        "nested: sandbox maintenance exhausted its retry budget: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
        "sandbox maintenance exhausted its retry budgetary limit: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
        "nested: sandbox maintenance commit remained unknown: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
        "sandbox maintenance commit remained unknownish: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
        "nested: sandbox maintenance commit exceeded its 30 second deadline; ambiguous commit: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
        "sandbox maintenance commit exceeded its 30 second deadline-ish; ambiguous commit: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
        "sandbox maintenance commit cannot start while 16 prior reconciliations remain pending; ambiguous commit: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
        "sandbox maintenance commit task terminated before completion; ambiguous commit: SYNTHETIC_DIAGNOSTIC_PAYLOAD",
    ] {
        assert_eq!(
            classify_error(&SandboxStoreError::Unavailable(message.into())),
            "unavailable_other"
        );
    }
}

#[test]
fn maintenance_error_classification_preserves_variants_without_payloads() {
    for (error, expected) in [
        (SandboxStoreError::Conflict, "conflict"),
        (SandboxStoreError::SequenceConflict, "sequence_conflict"),
        (
            SandboxStoreError::Corrupt("SYNTHETIC_DIAGNOSTIC_PAYLOAD".into()),
            "corrupt",
        ),
        (
            SandboxStoreError::InvalidTransaction("SYNTHETIC_DIAGNOSTIC_PAYLOAD"),
            "invalid_transaction",
        ),
    ] {
        let classification = classify_error(&error);
        assert_eq!(classification, expected);
        assert!(!classification.contains("SYNTHETIC_DIAGNOSTIC_PAYLOAD"));
    }
}
