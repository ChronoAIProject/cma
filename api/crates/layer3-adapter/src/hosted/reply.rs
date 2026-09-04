use crate::wire::{Acknowledgement, ResponseFrame, WireFailure, WireOperation};
use crate::AdapterError;

pub(super) fn expect_ack(response: ResponseFrame) -> crate::Result<()> {
    let acknowledgement = match response {
        ResponseFrame::RemoveWorktree(value)
        | ResponseFrame::MaterializeAgentHome(value)
        | ResponseFrame::CloseAgent(value)
        | ResponseFrame::Restore(value) => value,
        _ => {
            return Err(AdapterError::Rejected(
                "runtime returned no acknowledgement".into(),
            ))
        }
    };
    if acknowledgement == Acknowledgement::default() {
        Ok(())
    } else {
        Err(AdapterError::Rejected(
            "runtime refused to acknowledge the operation".into(),
        ))
    }
}

pub(super) fn response_operation(response: &ResponseFrame) -> WireOperation {
    match response {
        ResponseFrame::Install(_) => WireOperation::Install,
        ResponseFrame::PrepareWorkspace(_) => WireOperation::PrepareWorkspace,
        ResponseFrame::WorkspaceSetup(_) => WireOperation::WorkspaceSetup,
        ResponseFrame::RemoveWorkspace(_) => WireOperation::RemoveWorkspace,
        ResponseFrame::AddWorktree(_) => WireOperation::AddWorktree,
        ResponseFrame::RemoveWorktree(_) => WireOperation::RemoveWorktree,
        ResponseFrame::MaterializeAgentHome(_) => WireOperation::MaterializeAgentHome,
        ResponseFrame::OpenAgent(_) => WireOperation::OpenAgent,
        ResponseFrame::CloseAgent(_) => WireOperation::CloseAgent,
        ResponseFrame::CaptureAgent(_) => WireOperation::CaptureAgent,
        ResponseFrame::Checkpoint(_) => WireOperation::Checkpoint,
        ResponseFrame::Restore(_) => WireOperation::Restore,
        ResponseFrame::Credential(_) => WireOperation::Credential,
        ResponseFrame::Resources(_) => WireOperation::Resources,
        ResponseFrame::Failure(failure) => failure.operation,
    }
}

pub(super) fn failure_error(failure: WireFailure) -> AdapterError {
    let detail = format!("{}: {}", failure.code, failure.detail);
    if failure.retryable {
        AdapterError::Unavailable(detail)
    } else if failure.code == "at_capacity" {
        AdapterError::AtCapacity(detail)
    } else {
        AdapterError::Rejected(detail)
    }
}

pub(super) const fn operation_name(operation: WireOperation) -> &'static str {
    match operation {
        WireOperation::Install => "install",
        WireOperation::PrepareWorkspace => "prepare workspace",
        WireOperation::WorkspaceSetup => "workspace setup",
        WireOperation::RemoveWorkspace => "remove workspace",
        WireOperation::AddWorktree => "add worktree",
        WireOperation::RemoveWorktree => "remove worktree",
        WireOperation::MaterializeAgentHome => "materialize agent home",
        WireOperation::OpenAgent => "open agent",
        WireOperation::CloseAgent => "close agent",
        WireOperation::CaptureAgent => "capture agent",
        WireOperation::Checkpoint => "checkpoint",
        WireOperation::Restore => "restore",
        WireOperation::Credential => "credential",
        WireOperation::Resources => "resources",
    }
}

pub(super) fn validate_command_receipts(
    receipts: &[crate::CommandReceipt],
    expected: usize,
) -> crate::Result<()> {
    if receipts.len() != expected
        || receipts.iter().enumerate().any(|(index, receipt)| {
            receipt.index != index as u32
                || receipt.exit_code != 0
                || !valid_sha256(&receipt.stdout_sha256)
                || !valid_sha256(&receipt.stderr_sha256)
        })
    {
        return Err(AdapterError::Rejected(
            "runtime returned an invalid command receipt".into(),
        ));
    }
    Ok(())
}

pub(super) fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_failure_retryability_survives_the_wire() {
        let retryable = failure_error(WireFailure {
            operation: WireOperation::Install,
            code: "install_in_progress".into(),
            detail: "the first request is still running".into(),
            retryable: true,
        });
        assert!(matches!(retryable, AdapterError::Unavailable(_)));

        let terminal = failure_error(WireFailure {
            operation: WireOperation::Install,
            code: "install_replay_mismatch".into(),
            detail: "the request identity changed".into(),
            retryable: false,
        });
        assert!(matches!(terminal, AdapterError::Rejected(_)));
    }
}
