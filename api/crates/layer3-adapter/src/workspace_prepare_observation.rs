//! Bounded diagnostics for a single Workspace Prepare or Recovery call.
use std::{cell::Cell, future::Future};

use serde::{Deserialize, Serialize};

use crate::wire::{WireFailure, WireOperation};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    RuntimeInspection,
    PrepareRpc,
    ReceiptValidation,
    RecoveryRpc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    RepositoryConnectionRequired,
    RepositoryUnavailable,
    RepositoryRefNotFound,
    RepositoryArchiveUnavailable,
    RepositorySourceInvalid,
    RepositorySourceUnavailable,
    RepositorySourceIo,
    WorkspacePrepareIndeterminate,
    WorkspacePrepareOutcomeUnknown,
    WorkspaceRecoveryIneligible,
    WorkspaceRecoveryIoFailed,
    WorkspaceRecoveryPublicationPending,
    WorkspaceRecoveryAttemptFailed,
    Transport,
    Local,
    InvalidReceipt,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub stage: Stage,
    pub code: Code,
    /// Present only when received in a correlated wire failure.
    pub wire_retryable: Option<bool>,
}

tokio::task_local! {
    static OBSERVATION: Cell<Option<Observation>>;
    static NATIVE_RPC: WireOperation;
}

pub(crate) async fn native_rpc<T>(operation: WireOperation, work: impl Future<Output = T>) -> T {
    if OBSERVATION.try_with(|_| ()).is_ok() && rpc_stage(operation).is_some() {
        NATIVE_RPC.scope(operation, work).await
    } else {
        work.await
    }
}

pub(crate) fn native_transport() {
    let _ = NATIVE_RPC.try_with(|operation| transport(*operation));
}

pub(crate) fn inspection() {
    record(Observation {
        stage: Stage::RuntimeInspection,
        code: Code::Local,
        wire_retryable: None,
    });
}

pub async fn observe<T>(work: impl Future<Output = T>) -> (T, Option<Observation>) {
    OBSERVATION
        .scope(Cell::new(None), async {
            let result = work.await;
            (result, OBSERVATION.with(Cell::take))
        })
        .await
}

fn record(observation: Observation) {
    let _ = OBSERVATION.try_with(|slot| slot.set(Some(observation)));
}

fn rpc_stage(operation: WireOperation) -> Option<Stage> {
    match operation {
        WireOperation::PrepareWorkspace => Some(Stage::PrepareRpc),
        WireOperation::RecoverWorkspacePrepare => Some(Stage::RecoveryRpc),
        _ => None,
    }
}

pub(crate) fn transport(operation: WireOperation) {
    if let Some(stage) = rpc_stage(operation) {
        record(Observation {
            stage,
            code: Code::Transport,
            wire_retryable: None,
        });
    }
}

pub(crate) fn invalid_receipt() {
    record(Observation {
        stage: Stage::ReceiptValidation,
        code: Code::InvalidReceipt,
        wire_retryable: None,
    });
}

pub(crate) fn wire(failure: &WireFailure) {
    let Some(stage) = rpc_stage(failure.operation) else {
        return;
    };
    let code = match failure.code.as_str() {
        "repository_connection_required" => Code::RepositoryConnectionRequired,
        "repository_unavailable" => Code::RepositoryUnavailable,
        "repository_ref_not_found" => Code::RepositoryRefNotFound,
        "repository_archive_unavailable" => Code::RepositoryArchiveUnavailable,
        "repository_source_invalid" => Code::RepositorySourceInvalid,
        "repository_source_unavailable" => Code::RepositorySourceUnavailable,
        "repository_source_io" => Code::RepositorySourceIo,
        "workspace_prepare_indeterminate" => Code::WorkspacePrepareIndeterminate,
        "workspace_prepare_outcome_unknown" => Code::WorkspacePrepareOutcomeUnknown,
        "workspace_recovery_ineligible" => Code::WorkspaceRecoveryIneligible,
        "workspace_recovery_io_failed" => Code::WorkspaceRecoveryIoFailed,
        "workspace_recovery_publication_pending" => Code::WorkspaceRecoveryPublicationPending,
        "workspace_recovery_attempt_failed" => Code::WorkspaceRecoveryAttemptFailed,
        _ => Code::Unknown,
    };
    record(Observation {
        stage,
        code,
        wire_retryable: Some(failure.retryable),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn observes_only_fixed_codes_for_workspace_replies() {
        for (operation, code, stage, expected) in [
            (
                WireOperation::PrepareWorkspace,
                "repository_source_unavailable",
                Stage::PrepareRpc,
                Code::RepositorySourceUnavailable,
            ),
            (
                WireOperation::RecoverWorkspacePrepare,
                "workspace_recovery_ineligible",
                Stage::RecoveryRpc,
                Code::WorkspaceRecoveryIneligible,
            ),
            (
                WireOperation::PrepareWorkspace,
                "workspace_prepare_outcome_unknown suffix",
                Stage::PrepareRpc,
                Code::Unknown,
            ),
        ] {
            let (_, observed) = observe(async {
                wire(&WireFailure {
                    operation,
                    code: code.into(),
                    detail: "opaque runtime detail".into(),
                    retryable: false,
                });
            })
            .await;
            let observation = observed.unwrap();
            assert_eq!(
                observation,
                Observation {
                    stage,
                    code: expected,
                    wire_retryable: Some(false)
                }
            );
            let json = serde_json::to_string(&observation).unwrap();
            assert!(!json.contains("opaque runtime detail") && !json.contains("suffix"));
        }
        let (_, observed) = observe(native_rpc(WireOperation::PrepareWorkspace, async {
            native_transport();
        }))
        .await;
        assert_eq!(
            observed,
            Some(Observation {
                stage: Stage::PrepareRpc,
                code: Code::Transport,
                wire_retryable: None,
            })
        );
        let (_, observed) = observe(async { transport(WireOperation::Install) }).await;
        assert_eq!(observed, None);
    }
}
