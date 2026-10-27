use crate::wire::{Acknowledgement, ResponseFrame, WireFailure, WireOperation};
use crate::AdapterError;

pub(super) fn expect_ack(response: ResponseFrame) -> crate::Result<()> {
    let acknowledgement = match response {
        ResponseFrame::Restore(value) => value,
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
        ResponseFrame::UploadAgentAttachment(_) => WireOperation::UploadAgentAttachment,
        ResponseFrame::ListAgentFiles(_) => WireOperation::ListAgentFiles,
        ResponseFrame::ReadAgentFile(_) => WireOperation::ReadAgentFile,
        ResponseFrame::WriteAgentFile(_) => WireOperation::WriteAgentFile,
        ResponseFrame::DeleteAgentFile(_) => WireOperation::DeleteAgentFile,
        ResponseFrame::CacheSkillArchive { .. } => WireOperation::CacheSkillArchive,
        ResponseFrame::RuntimeStatus(_) => WireOperation::RuntimeStatus,
        ResponseFrame::ToolProxyStatus(_) => WireOperation::ToolProxyStatus,
        ResponseFrame::WorkspaceRemovalSupport(_) => WireOperation::WorkspaceRemovalSupport,
        ResponseFrame::ReconcileWorkspaceRemoval(_) => WireOperation::ReconcileWorkspaceRemoval,
        ResponseFrame::Install(_) => WireOperation::Install,
        ResponseFrame::BootstrapRestore(_) => WireOperation::BootstrapRestore,
        ResponseFrame::PrepareWorkspace(_) => WireOperation::PrepareWorkspace,
        ResponseFrame::WorkspaceSetup(_) => WireOperation::WorkspaceSetup,
        ResponseFrame::RemoveWorkspace(_) => WireOperation::RemoveWorkspace,
        ResponseFrame::AddWorktree(_) => WireOperation::AddWorktree,
        ResponseFrame::RemoveWorktree(_) => WireOperation::RemoveWorktree,
        ResponseFrame::MaterializeAgentHome(_) => WireOperation::MaterializeAgentHome,
        ResponseFrame::InstallAgentSkill(_) => WireOperation::InstallAgentSkill,
        ResponseFrame::RemoveAgentHome(_) => WireOperation::RemoveAgentHome,
        ResponseFrame::OpenAgent(_) => WireOperation::OpenAgent,
        ResponseFrame::CloseAgent(_) => WireOperation::CloseAgent,
        ResponseFrame::CaptureAgent(_) => WireOperation::CaptureAgent,
        ResponseFrame::Checkpoint(_) => WireOperation::Checkpoint,
        ResponseFrame::CheckpointForTeardown(_) => WireOperation::CheckpointForTeardown,
        ResponseFrame::ResumeCheckpointWriters(_) => WireOperation::ResumeCheckpointWriters,
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
    } else if failure.code == "attachment_upload_conflict" {
        AdapterError::OperationIdentityConflict(detail)
    } else if failure.code == "at_capacity" {
        AdapterError::AtCapacity(detail)
    } else {
        AdapterError::Rejected(detail)
    }
}

pub(super) const fn operation_name(operation: WireOperation) -> &'static str {
    match operation {
        WireOperation::UploadAgentAttachment => "upload agent attachment",
        WireOperation::ListAgentFiles => "list agent files",
        WireOperation::ReadAgentFile => "read agent file",
        WireOperation::WriteAgentFile => "write agent file",
        WireOperation::DeleteAgentFile => "delete agent file",
        WireOperation::CacheSkillArchive => "cache skill archive",
        WireOperation::RuntimeStatus => "runtime status",
        WireOperation::ToolProxyStatus => "runtime tool endpoint status",
        WireOperation::WorkspaceRemovalSupport => "Workspace removal support",
        WireOperation::ReconcileWorkspaceRemoval => "Workspace removal observation",
        WireOperation::Install => "install",
        WireOperation::BootstrapRestore => "restore bootstrap",
        WireOperation::PrepareWorkspace => "prepare workspace",
        WireOperation::WorkspaceSetup => "workspace setup",
        WireOperation::RemoveWorkspace => "remove workspace",
        WireOperation::AddWorktree => "add worktree",
        WireOperation::RemoveWorktree => "remove worktree",
        WireOperation::MaterializeAgentHome => "materialize agent home",
        WireOperation::InstallAgentSkill => "install agent skill",
        WireOperation::RemoveAgentHome => "remove agent home",
        WireOperation::OpenAgent => "open agent",
        WireOperation::CloseAgent => "close agent",
        WireOperation::CaptureAgent => "capture agent",
        WireOperation::Checkpoint => "checkpoint",
        WireOperation::CheckpointForTeardown => "checkpoint for teardown",
        WireOperation::ResumeCheckpointWriters => "resume checkpoint writers",
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
            receipt.index != u32::try_from(index).unwrap_or(u32::MAX)
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
    value.len() == 64 && value.bytes().all(lower_hex)
}

pub(super) fn valid_git_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(lower_hex)
}

pub(super) fn valid_checkpoint_reference(reference: &str, sha256: &str) -> bool {
    valid_sha256(sha256)
        && reference
            .strip_prefix("home-")
            .and_then(|value| value.strip_suffix(".cma"))
            == Some(sha256)
}

fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
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

    #[test]
    fn hashes_commits_and_checkpoint_references_are_canonical() {
        let sha256 = "a".repeat(64);
        assert!(valid_sha256(&sha256));
        assert!(!valid_sha256(&"A".repeat(64)));
        assert!(valid_git_commit(&"b".repeat(40)));
        assert!(valid_git_commit(&"b".repeat(64)));
        assert!(!valid_git_commit(&"B".repeat(40)));
        assert!(valid_checkpoint_reference(
            &format!("home-{sha256}.cma"),
            &sha256
        ));
        assert!(!valid_checkpoint_reference(
            &format!("renamed-{sha256}.cma"),
            &sha256
        ));
    }
}
