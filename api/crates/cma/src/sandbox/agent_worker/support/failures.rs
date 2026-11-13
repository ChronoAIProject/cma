//! Map dependency failures without changing their durable diagnostics.
use super::*;

pub(in crate::sandbox::agent_worker) fn adapter_failure(error: AdapterError) -> WorkerFailure {
    match error {
        AdapterError::InputAuthorityUnavailable(detail)
        | AdapterError::PromptNotWritten(detail)
        | AdapterError::Unavailable(detail) => {
            WorkerFailure::retryable("agent_runtime_unavailable", detail)
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

pub(in crate::sandbox::agent_worker) fn cleanup_failure(error: layer3_adapter::AdapterError) -> WorkerFailure {
    if matches!(error, layer3_adapter::AdapterError::RuntimeLedgerHold(_)) {
        adapter_failure(error)
    } else {
        WorkerFailure::retryable("agent_cleanup_unavailable", error.to_string())
    }
}

pub(in crate::sandbox::agent_worker) fn skill_failure(error: AgentSkillResolutionError) -> WorkerFailure {
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
