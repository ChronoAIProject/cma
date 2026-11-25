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
        ResponseFrame::Browser(_) => WireOperation::Browser,
        ResponseFrame::BrowserControl(_) => WireOperation::BrowserControl,
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
        ResponseFrame::RecoverWorkspacePrepare(_) => WireOperation::RecoverWorkspacePrepare,
        ResponseFrame::WorkspaceSetup(_) => WireOperation::WorkspaceSetup,
        ResponseFrame::RemoveWorkspace(_) => WireOperation::RemoveWorkspace,
        ResponseFrame::PrepareAgentResources(_) => WireOperation::PrepareAgentResources,
        ResponseFrame::AddWorktree(_) => WireOperation::AddWorktree,
        ResponseFrame::RemoveWorktree(_) => WireOperation::RemoveWorktree,
        ResponseFrame::MaterializeAgentHome(_) => WireOperation::MaterializeAgentHome,
        ResponseFrame::InstallAgentSkill(_) => WireOperation::InstallAgentSkill,
        ResponseFrame::RemoveAgentHome(_) => WireOperation::RemoveAgentHome,
        ResponseFrame::OpenAgent(_) => WireOperation::OpenAgent,
        ResponseFrame::CloseAgent(_) => WireOperation::CloseAgent,
        ResponseFrame::ProveAgentProcessClosed(_) => WireOperation::ProveAgentProcessClosed,
        ResponseFrame::FenceAgentRestart(_) => WireOperation::FenceAgentRestart,
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
    if ledger_hold(&failure) {
        AdapterError::RuntimeLedgerHold(detail)
    } else if failure.retryable {
        AdapterError::Unavailable(detail)
    } else if matches!(
        failure.code.as_str(),
        "attachment_upload_conflict" | "agent_skill_upgrade_conflict"
    ) {
        AdapterError::OperationIdentityConflict(detail)
    } else if failure.operation == WireOperation::InstallAgentSkill
        && matches!(
            failure.code.as_str(),
            "agent_skill_upgrade_modified"
                | "agent_skill_upgrade_previous_unavailable"
                | "agent_skill_upgrade_retention_full"
                | "agent_skill_upgrade_recovery_conflict"
                | "agent_skill_upgrade_preparation_exhausted"
        )
    {
        use crate::AgentSkillUpgradeRefusal as Refusal;
        AdapterError::SkillUpgradeRefused(match failure.code.as_str() {
            "agent_skill_upgrade_modified" => Refusal::Modified,
            "agent_skill_upgrade_previous_unavailable" => Refusal::PreviousUnavailable,
            "agent_skill_upgrade_retention_full" => Refusal::RetentionFull,
            "agent_skill_upgrade_recovery_conflict" => Refusal::RecoveryConflict,
            _ => Refusal::PreparationExhausted,
        })
    } else if failure.code == "at_capacity" {
        AdapterError::AtCapacity(detail)
    } else {
        AdapterError::Rejected(detail)
    }
}

// Only exact code/detail pairs prove a capacity, storage or format hold.
// Do not normalize, trim or search details, or mask other corruption as a hold.
pub(super) fn ledger_hold(failure: &WireFailure) -> bool {
    match failure.code.as_str() {
        "mutation_ledger_capacity" => {
            failure.detail == "runtime mutation ledger capacity is exhausted"
        }
        "mutation_ledger_storage_hold" => {
            failure.detail == "runtime mutation ledger storage requires verification"
        }
        "mutation_ledger_busy" => {
            failure.detail == "runtime mutation ledger worker capacity is exhausted"
        }
        "mutation_ledger_writer_held" => {
            failure.detail == "runtime mutation ledger has another writer"
        }
        "mutation_ledger_format_hold" => {
            failure.detail == "runtime mutation ledger format requires verification"
        }
        "mutation_ledger_import_held" => {
            failure.detail == "imported runtime mutation ledger requires reviewed release authority"
        }
        "mutation_authority_corrupt" => matches!(
            failure.detail.as_str(),
            "runtime mutation authority contains too many records"
                | "mutation authority contains too many records"
        ),
        "mutation_journal_corrupt" => {
            failure.detail == "runtime mutation journal contains too many entries"
        }
        _ => false,
    }
}

pub(super) const fn operation_name(operation: WireOperation) -> &'static str {
    match operation {
        WireOperation::Browser => "observe agent browser",
        WireOperation::BrowserControl => "control agent browser",
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
        WireOperation::RecoverWorkspacePrepare => "recover Workspace preparation",
        WireOperation::WorkspaceSetup => "workspace setup",
        WireOperation::RemoveWorkspace => "remove workspace",
        WireOperation::PrepareAgentResources => "prepare Agent resources",
        WireOperation::AddWorktree => "add worktree",
        WireOperation::RemoveWorktree => "remove worktree",
        WireOperation::MaterializeAgentHome => "materialize agent home",
        WireOperation::InstallAgentSkill => "install agent skill",
        WireOperation::RemoveAgentHome => "remove agent home",
        WireOperation::OpenAgent => "open agent",
        WireOperation::CloseAgent => "close agent",
        WireOperation::ProveAgentProcessClosed => "prove agent process closed",
        WireOperation::FenceAgentRestart => "fence agent restart",
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
    fn skill_upgrade_refusals_are_typed_only_for_terminal_skill_operations() {
        for reason in [
            crate::AgentSkillUpgradeRefusal::Modified,
            crate::AgentSkillUpgradeRefusal::PreviousUnavailable,
            crate::AgentSkillUpgradeRefusal::RetentionFull,
            crate::AgentSkillUpgradeRefusal::RecoveryConflict,
            crate::AgentSkillUpgradeRefusal::PreparationExhausted,
        ] {
            let mut failure = WireFailure {
                operation: WireOperation::InstallAgentSkill,
                code: reason.code().into(),
                detail: "owner action required".into(),
                retryable: false,
            };
            assert_eq!(
                failure_error(failure.clone()),
                AdapterError::SkillUpgradeRefused(reason)
            );
            failure.retryable = true;
            assert!(matches!(
                failure_error(failure.clone()),
                AdapterError::Unavailable(_)
            ));
            failure.retryable = false;
            failure.operation = WireOperation::Install;
            assert!(matches!(failure_error(failure), AdapterError::Rejected(_)));
        }
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
