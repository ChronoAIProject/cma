//! Map dependency failures without changing their durable diagnostics.
use super::*;

pub(in crate::sandbox::agent_worker) fn adapter_failure(error: AdapterError) -> WorkerFailure {
    match error {
        AdapterError::InputAuthorityUnavailable(detail)
        | AdapterError::PromptNotWritten(detail)
        | AdapterError::Unavailable(detail) => {
            let external = crate::sandbox::work_retry::replaced_runtime(&detail);
            let mut failure = WorkerFailure::retryable("agent_runtime_unavailable", detail);
            if external {
                failure.retry_kind = WorkRetryFailure::ExternalAction;
            }
            failure
        }
        AdapterError::NativeClaimNotReady(reason) => {
            WorkerFailure::retryable("agent_runtime_not_ready", reason.to_string())
        }
        AdapterError::RuntimeLedgerHold(_) => WorkerFailure::retryable(
            layer2_core::sandbox::store::AGENT_RUNTIME_LEDGER_HOLD,
            layer2_core::sandbox::store::AGENT_RUNTIME_LEDGER_HOLD_DETAIL,
        ),
        AdapterError::SkillUpgradeRefused(reason) => {
            WorkerFailure::terminal(reason.code(), reason.to_string())
        }
        AdapterError::AtCapacity(detail) => WorkerFailure::terminal("agent_resource_quota", detail),
        AdapterError::Rejected(detail) | AdapterError::OperationIdentityConflict(detail) => {
            WorkerFailure::terminal("agent_runtime_rejected", detail)
        }
        AdapterError::Unsupported(detail) => {
            WorkerFailure::terminal("agent_runtime_unsupported", detail)
        }
        AdapterError::RuntimeContractMismatch(error) => {
            WorkerFailure::terminal("agent_runtime_contract_mismatch", error.to_string())
        }
    }
}

pub(in crate::sandbox::agent_worker) fn cleanup_failure(
    error: layer3_adapter::AdapterError,
) -> WorkerFailure {
    if matches!(error, layer3_adapter::AdapterError::RuntimeLedgerHold(_)) {
        adapter_failure(error)
    } else {
        let external = matches!(&error, AdapterError::Unavailable(detail)
            if crate::sandbox::work_retry::replaced_runtime(detail));
        let mut failure = WorkerFailure::retryable("agent_cleanup_unavailable", error.to_string());
        if external {
            failure.retry_kind = WorkRetryFailure::ExternalAction;
        }
        failure
    }
}

pub(in crate::sandbox::agent_worker) fn skill_failure(
    error: AgentSkillResolutionError,
) -> WorkerFailure {
    match error {
        AgentSkillResolutionError::Unavailable(detail) => {
            WorkerFailure::retryable("agent_skill_source_unavailable", detail)
        }
        AgentSkillResolutionError::NotFound(detail) => {
            WorkerFailure::terminal("agent_skill_not_found", detail)
        }
        AgentSkillResolutionError::Invalid(detail) => {
            WorkerFailure::terminal("agent_skill_invalid", detail)
        }
    }
}

pub(super) fn parent_store_failure(error: SandboxStoreError) -> WorkerFailure {
    match error {
        SandboxStoreError::Unavailable(detail) => {
            WorkerFailure::retryable("agent_store_unavailable", detail)
        }
        error => WorkerFailure::terminal("agent_parent_corrupt", error.to_string()),
    }
}

pub(super) fn missing_parent() -> WorkerFailure {
    WorkerFailure::terminal("agent_parent_missing", "an agent parent record is missing")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_retry_classification_preserves_diagnostics_and_terminal_semantics() {
        for message in [
            "bound runtime process was replaced; durable recovery is pending",
            "retiring native runtime process was replaced",
        ] {
            for failure in [
                adapter_failure(AdapterError::Unavailable(message.into())),
                cleanup_failure(AdapterError::Unavailable(message.into())),
            ] {
                assert_eq!(failure.retry_kind, WorkRetryFailure::ExternalAction);
                assert!(failure.retryable);
                assert!(failure.detail.contains(message));
            }
        }
        assert_eq!(
            adapter_failure(AdapterError::Unavailable("network".into())).retry_kind,
            WorkRetryFailure::Transient
        );
        assert!(!adapter_failure(AdapterError::Rejected("refused".into())).retryable);
        assert_eq!(
            adapter_failure(AdapterError::RuntimeLedgerHold("hold".into())).retry_kind,
            WorkRetryFailure::Transient
        );
    }
}
