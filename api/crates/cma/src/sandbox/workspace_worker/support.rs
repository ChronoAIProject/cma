use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{TypedFailure, Workspace};
use layer3_adapter::{
    AdapterError, SetupReceipt, WorkspaceReceipt, WorkspaceRequest, WorkspaceSetupRequest,
};

pub(super) fn claimed_fence(workspace: &Workspace, holder: &str) -> Result<u64, SandboxStoreError> {
    workspace
        .work
        .lease
        .as_ref()
        .filter(|lease| lease.holder == holder)
        .map(|lease| lease.fence)
        .ok_or(SandboxStoreError::Conflict)
}

fn valid_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64) && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn valid_prepare_receipt(
    request: &WorkspaceRequest,
    receipt: &WorkspaceReceipt,
    expected_path: &str,
) -> bool {
    receipt.operation_id == request.operation_id
        && receipt.request_sha256 == request.request_sha256
        && receipt.workspace_id == request.workspace_id
        && receipt.name == request.name
        && receipt.path == expected_path
        && valid_commit(&receipt.resolved_commit)
        && match &request.source {
            layer3_adapter::WorkspaceSource::RepositorySnapshot { reference, .. } => {
                receipt.source_commit.as_deref().is_some_and(valid_commit)
                    && (request.repository_connection_id.is_none()
                        || &receipt.source_commit == reference)
            }
            layer3_adapter::WorkspaceSource::PublicRepositorySnapshot { commit, .. } => {
                receipt.source_commit.as_ref() == Some(commit)
            }
            _ => receipt.source_commit.is_none(),
        }
}

pub(super) fn valid_setup_receipt(request: &WorkspaceSetupRequest, receipt: &SetupReceipt) -> bool {
    receipt.operation_id == request.operation_id
        && receipt.request_sha256 == request.request_sha256
        && receipt.workspace_id == request.workspace_id
        && receipt.commands.len() == request.commands.len()
        && receipt.commands.iter().enumerate().all(|(index, command)| {
            command.index == index as u32
                && command.exit_code == 0
                && valid_sha256(&command.stdout_sha256)
                && valid_sha256(&command.stderr_sha256)
        })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug)]
pub(super) struct WorkerFailure {
    pub(super) observation: Option<layer3_adapter::workspace_prepare_observation::Observation>,
    pub(super) class: &'static str,
    pub(super) detail: &'static str,
    pub(super) retryable: bool,
}

impl WorkerFailure {
    pub(super) fn retryable(detail: &'static str) -> Self {
        Self {
            observation: None,
            class: "workspace_dependency_unavailable",
            detail,
            retryable: true,
        }
    }

    pub(super) fn terminal(class: &'static str, detail: &'static str) -> Self {
        Self {
            observation: None,
            class,
            detail,
            retryable: false,
        }
    }

    pub(super) fn invalid_receipt() -> Self {
        Self::terminal(
            "workspace_runtime_receipt_invalid",
            "runtime returned invalid workspace evidence",
        )
    }

    pub(super) fn typed(&self) -> TypedFailure {
        TypedFailure {
            class: self.class.into(),
            detail: self.detail.into(),
        }
    }
}

pub(super) fn failure(error: AdapterError) -> WorkerFailure {
    match error {
        AdapterError::AccessRevoked => WorkerFailure::terminal(
            "workspace_access_revoked",
            "current runtime authority was revoked",
        ),
        AdapterError::InputAuthorityUnavailable(_)
        | AdapterError::PromptNotWritten(_)
        | AdapterError::NativePoolDefinitionAbsent { .. }
        | AdapterError::NativeClaimNotReady(_)
        | AdapterError::RuntimeStartupHold(_)
        | AdapterError::Unavailable(_) => {
            WorkerFailure::retryable("workspace runtime is temporarily unavailable")
        }
        AdapterError::RuntimeLedgerHold(_) => {
            WorkerFailure::retryable("runtime mutation ledger capacity requires recovery")
        }
        AdapterError::RuntimeCapacity(_) => WorkerFailure::retryable("runtime_request_capacity_exhausted; admitted operation retained"),
        AdapterError::AtCapacity(_) => WorkerFailure::terminal(
            "workspace_resource_quota",
            "workspace runtime has no available capacity",
        ),
        AdapterError::Rejected(detail) if detail.starts_with("repository_ref_not_found:") => WorkerFailure::terminal(
            "repository_ref_not_found", "The requested repository ref could not be found. Start another task to change the ref.",
        ),
        AdapterError::Rejected(detail) if detail.starts_with("repository_unavailable:") => WorkerFailure::terminal(
            "repository_unavailable", "The repository is unavailable to the connected account. Check repository access before starting another task.",
        ),
        AdapterError::Rejected(detail) if detail.starts_with("repository_archive_unavailable:") => WorkerFailure::terminal(
            "repository_archive_unavailable", "The repository archive is unavailable. Check access to the requested revision before starting another task.",
        ),
        AdapterError::Rejected(detail) if detail.starts_with("repository_connection_required:") => WorkerFailure::terminal(
            "repository_connection_required", "GitHub authorization does not allow repository preparation. Check the existing GitHub connection before starting another task.",
        ),
        AdapterError::Rejected(detail) if detail.starts_with("repository_source_invalid:") => WorkerFailure::terminal(
            "repository_source_invalid", "Repository files could not be safely prepared. The task has not run. Check source contents before starting another task.",
        ),
        AdapterError::Rejected(_)
        | AdapterError::AdmittedSandboxSpecDrift
        | AdapterError::NativePoolConfigurationDrift
        | AdapterError::DesktopFailed
        | AdapterError::SkillUpgradeRefused(_)
        | AdapterError::OperationIdentityConflict(_)
        | AdapterError::Unsupported(_) => WorkerFailure::terminal(
            "workspace_runtime_rejected",
            "workspace runtime rejected the operation",
        ),
        AdapterError::RuntimeContractMismatch(_) => WorkerFailure::terminal(
            "workspace_runtime_contract_mismatch",
            "workspace runtime contract was not admitted",
        ),
    }
}

#[cfg(test)]
mod tests {
    use layer3_adapter::{CommandReceipt, WorkspaceSource};

    use super::*;

    fn sha() -> String {
        "a".repeat(64)
    }

    #[test]
    fn prepare_receipts_bind_the_entire_request_identity() {
        let request = WorkspaceRequest::new(
            "workspace:wks-one:prepare:v1",
            "wks-one",
            "project",
            WorkspaceSource::Init,
        );
        let mut receipt = WorkspaceReceipt {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            workspace_id: request.workspace_id.clone(),
            name: request.name.clone(),
            path: "/home/cma/project".into(),
            resolved_commit: "a".repeat(40),
            source_commit: None,
        };
        assert!(valid_prepare_receipt(
            &request,
            &receipt,
            "/home/cma/project"
        ));
        receipt.workspace_id = "wks-other".into();
        assert!(!valid_prepare_receipt(
            &request,
            &receipt,
            "/home/cma/project"
        ));
    }

    #[test]
    fn setup_receipts_require_exact_successful_command_proof() {
        let request = WorkspaceSetupRequest::new(
            "workspace:wks-one:setup:v1",
            "wks-one",
            "project",
            vec!["make setup".into()],
        );
        let mut receipt = SetupReceipt {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            workspace_id: request.workspace_id.clone(),
            commands: vec![CommandReceipt {
                index: 0,
                exit_code: 0,
                stdout_sha256: sha(),
                stderr_sha256: sha(),
            }],
        };
        assert!(valid_setup_receipt(&request, &receipt));
        receipt.commands[0].index = 1;
        assert!(!valid_setup_receipt(&request, &receipt));
    }
}
