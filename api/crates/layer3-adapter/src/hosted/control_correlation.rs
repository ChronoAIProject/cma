use sha2::{Digest as _, Sha256};

use crate::wire::{RequestFrame, ResponseFrame};
use crate::{AdapterError, CommandReceipt, WriterState};

pub(super) fn validate(request: &RequestFrame, response: &ResponseFrame) -> crate::Result<()> {
    let correlated = match (request, response) {
        (RequestFrame::BrowserControl(request), ResponseFrame::BrowserControl(receipt)) => {
            request.agent_id == receipt.agent_id
                && request.boot_id == receipt.boot_id
                && request
                    .context_id
                    .as_ref()
                    .is_none_or(|value| *value == receipt.context_id)
                && receipt.control.operation_id == request.operation_id
                && uuid::Uuid::parse_str(&receipt.context_id).is_ok()
                && receipt.control.valid()
        }
        (RequestFrame::Browser(request), ResponseFrame::Browser(receipt)) => {
            request.agent_id == receipt.agent_id
                && request.boot_id == receipt.boot_id
                && receipt.observation.valid()
                && request.context_id.as_ref().is_none_or(|context| {
                    receipt.observation.context_id.as_ref() == Some(context)
                        || (receipt.observation.frame.is_none()
                            && matches!(
                                receipt.observation.state,
                                crate::BrowserState::Paused
                                    | crate::BrowserState::NotEnabled
                                    | crate::BrowserState::Unsupported
                            ))
                })
        }
        (
            RequestFrame::UploadAgentAttachment(upload),
            ResponseFrame::UploadAgentAttachment(receipt),
        ) => receipt.validate_for(&upload.request).is_ok(),
        (RequestFrame::ListAgentFiles(request), ResponseFrame::ListAgentFiles(receipt)) => {
            receipt.path == request.path
                && receipt.entries.len() <= crate::MAX_AGENT_FILE_ENTRIES
                && receipt.entries.iter().all(|entry| {
                    crate::valid_agent_file_path(&entry.path)
                        && !entry.name.is_empty()
                        && !entry.name.contains('/')
                        && entry.name != "."
                        && entry.name != ".."
                        && entry.path
                            == if request.path.is_empty() {
                                entry.name.clone()
                            } else {
                                format!("{}/{}", request.path, entry.name)
                            }
                })
        }
        (RequestFrame::ReadAgentFile(request), ResponseFrame::ReadAgentFile(receipt)) => {
            receipt.path == request.path
                && receipt.size_bytes <= request.max_bytes
                && receipt.content().is_ok()
        }
        (RequestFrame::WriteAgentFile(request), ResponseFrame::WriteAgentFile(receipt)) => {
            receipt.path == request.path
                && receipt.size_bytes <= crate::MAX_AGENT_FILE_BYTES
                && valid_sha256(&receipt.sha256)
                && request.content().is_ok_and(|bytes| {
                    receipt.size_bytes == bytes.len() as u64
                        && receipt.sha256 == format!("{:x}", Sha256::digest(bytes))
                })
        }
        (RequestFrame::DeleteAgentFile(request), ResponseFrame::DeleteAgentFile(receipt)) => {
            receipt.path == request.path
        }
        (RequestFrame::InstallAgentSkill(request), ResponseFrame::InstallAgentSkill(receipt)) => {
            receipt.authority_fence == request.authority_fence
                && receipt.receipt.matches(&request.request)
        }
        (RequestFrame::CacheSkillArchive(request), ResponseFrame::CacheSkillArchive { sha256 }) => {
            sha256 == &request.sha256
        }
        (RequestFrame::ToolProxyStatus(request), ResponseFrame::ToolProxyStatus(receipt)) => {
            receipt.validate_for(request).is_ok()
        }
        (RequestFrame::RuntimeStatus(request), ResponseFrame::RuntimeStatus(receipt)) => {
            receipt.validate_for(request).is_ok()
        }
        (
            RequestFrame::WorkspaceRemovalSupport(request),
            ResponseFrame::WorkspaceRemovalSupport(receipt),
        ) => receipt.validate_for(request).is_ok(),
        (
            RequestFrame::ReconcileWorkspaceRemoval(request),
            ResponseFrame::ReconcileWorkspaceRemoval(receipt),
        ) => receipt.validate_for(request).is_ok(),
        (RequestFrame::Install(request), ResponseFrame::Install(receipt)) => {
            receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && valid_commands(&receipt.commands, request.commands.len())
        }
        (RequestFrame::BootstrapRestore(request), ResponseFrame::BootstrapRestore(receipt)) => {
            receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && receipt.restore_request_sha256 == request.checkpoint.computed_sha256()
                && receipt.installation_inherited
        }
        (RequestFrame::PrepareWorkspace(request), ResponseFrame::PrepareWorkspace(receipt)) => {
            receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && receipt.workspace_id == request.workspace_id
                && receipt.name == request.name
                && !receipt.path.is_empty()
                && valid_git_commit(&receipt.resolved_commit)
        }
        (
            RequestFrame::RecoverWorkspacePrepare(frame),
            ResponseFrame::RecoverWorkspacePrepare(receipt),
        ) => {
            let request = &frame.request;
            receipt.prepared.operation_id == request.operation_id
                && receipt.prepared.request_sha256 == request.request_sha256
                && receipt.prepared.workspace_id == request.original.workspace_id
                && receipt.prepared.name == request.original.name
                && receipt.preserved_path == request.preserved_path()
                && valid_sha256(&receipt.original_journal_sha256)
                && valid_git_commit(&receipt.prepared.resolved_commit)
        }
        (RequestFrame::WorkspaceSetup(request), ResponseFrame::WorkspaceSetup(receipt)) => {
            receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && receipt.workspace_id == request.workspace_id
                && valid_commands(&receipt.commands, request.commands.len())
        }
        (RequestFrame::RemoveWorkspace(request), ResponseFrame::RemoveWorkspace(receipt)) => {
            receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && receipt.workspace_id == request.workspace_id
        }
        (
            RequestFrame::PrepareAgentResources(request),
            ResponseFrame::PrepareAgentResources(receipt),
        ) => {
            let fence = request.authority_fence;
            receipt.authority_fence == fence
                && validate(
                    &RequestFrame::AddWorktree(crate::wire::FencedRequest::new(
                        fence,
                        request.request.worktree.clone(),
                    )),
                    &ResponseFrame::AddWorktree(crate::wire::FencedReceipt::new(
                        fence,
                        receipt.receipt.worktree.clone(),
                    )),
                )
                .is_ok()
                && validate(
                    &RequestFrame::MaterializeAgentHome(crate::wire::FencedRequest::new(
                        fence,
                        request.request.home.clone(),
                    )),
                    &ResponseFrame::MaterializeAgentHome(crate::wire::FencedReceipt::new(
                        fence,
                        receipt.receipt.home.clone(),
                    )),
                )
                .is_ok()
        }
        (RequestFrame::AddWorktree(request), ResponseFrame::AddWorktree(receipt)) => {
            receipt.authority_fence == request.authority_fence
                && receipt.receipt.operation_id == request.request.operation_id
                && receipt.receipt.request_sha256 == request.request.request_sha256
                && receipt.receipt.workspace == request.request.workspace
                && receipt.receipt.name == request.request.name
                && !receipt.receipt.path.is_empty()
                && !receipt.receipt.branch.is_empty()
                && valid_git_commit(&receipt.receipt.base_commit)
        }
        (RequestFrame::RemoveWorktree(request), ResponseFrame::RemoveWorktree(receipt)) => {
            receipt.authority_fence == request.authority_fence
                && receipt.receipt.operation_id == request.request.operation_id
                && receipt.receipt.request_sha256 == request.request.request_sha256
                && receipt.receipt.workspace == request.request.workspace
                && receipt.receipt.name == request.request.name
        }
        (
            RequestFrame::MaterializeAgentHome(request),
            ResponseFrame::MaterializeAgentHome(receipt),
        ) => {
            receipt.authority_fence == request.authority_fence
                && receipt.receipt.operation_id == request.request.operation_id
                && receipt.receipt.request_sha256 == request.request.request_sha256
                && receipt.receipt.agent_id == request.request.agent_id
                && !receipt.receipt.path.is_empty()
        }
        (RequestFrame::RemoveAgentHome(request), ResponseFrame::RemoveAgentHome(receipt)) => {
            receipt.authority_fence == request.authority_fence
                && receipt.receipt.operation_id == request.request.operation_id
                && receipt.receipt.request_sha256 == request.request.request_sha256
                && receipt.receipt.agent_id == request.request.agent_id
        }
        (RequestFrame::OpenAgent(request), ResponseFrame::OpenAgent(receipt)) => {
            receipt.authority_fence == request.authority_fence
                && receipt.receipt.operation_id == request.request.operation_id
                && receipt.receipt.request_sha256 == request.request.request_sha256
                && receipt.receipt.agent_id == request.request.agent_id
                && receipt.receipt.cwd == request.request.cwd
                && receipt.receipt.home == request.request.home
                && receipt.receipt.conversation_id.is_none()
        }
        (RequestFrame::CloseAgent(request), ResponseFrame::CloseAgent(receipt)) => {
            receipt.authority_fence == request.authority_fence
                && receipt.receipt.operation_id == request.request.operation_id
                && receipt.receipt.request_sha256 == request.request.request_sha256
                && receipt.receipt.agent_id == request.request.agent_id
        }
        (
            RequestFrame::ProveAgentProcessClosed(request),
            ResponseFrame::ProveAgentProcessClosed(proof),
        ) => request == proof,
        (RequestFrame::FenceAgentRestart(request), ResponseFrame::FenceAgentRestart(receipt)) => {
            request == receipt
        }
        (RequestFrame::CaptureAgent(_), ResponseFrame::CaptureAgent(snapshot)) => {
            !snapshot.conversation_id.is_empty()
                && valid_sha256(&snapshot.archive_sha256)
                && hex(&Sha256::digest(&snapshot.archive)) == snapshot.archive_sha256
        }
        (RequestFrame::Checkpoint(request), ResponseFrame::Checkpoint(receipt)) => {
            receipt.epoch == request.epoch
                && valid_checkpoint(&receipt.object_reference, &receipt.sha256)
                && receipt.writers == WriterState::Resumed
        }
        (
            RequestFrame::CheckpointForTeardown(request),
            ResponseFrame::CheckpointForTeardown(receipt),
        ) => {
            receipt.checkpoint.epoch == request.checkpoint.epoch
                && valid_checkpoint(
                    &receipt.checkpoint.object_reference,
                    &receipt.checkpoint.sha256,
                )
                && receipt.checkpoint.writers == WriterState::Held
                && crate::writer_resume_tokens_match(
                    &receipt.writer_resume_token,
                    &request.writer_resume_token,
                )
        }
        (
            RequestFrame::ResumeCheckpointWriters(request),
            ResponseFrame::ResumeCheckpointWriters(receipt),
        ) => {
            receipt.writers_resumed
                && crate::writer_resume_tokens_match(
                    &receipt.writer_resume_token,
                    &request.writer_resume_token,
                )
        }
        (RequestFrame::Restore(_), ResponseFrame::Restore(receipt)) => receipt.accepted,
        (RequestFrame::Credential(request), ResponseFrame::Credential(receipt))
        | (
            RequestFrame::CredentialForReceiver {
                credential: request,
                ..
            },
            ResponseFrame::Credential(receipt),
        ) => receipt.operation_id == request.operation_id && receipt.installed,
        (RequestFrame::Resources, ResponseFrame::Resources(reading)) => reading
            .stall_permille
            .is_none_or(|permille| permille <= 1_000),
        (_, ResponseFrame::Failure(failure)) => failure.operation == request.operation(),
        _ => false,
    };
    if correlated {
        Ok(())
    } else {
        Err(AdapterError::Rejected(format!(
            "runtime returned an uncorrelated {:?} response",
            request.operation()
        )))
    }
}

fn valid_commands(receipts: &[CommandReceipt], expected: usize) -> bool {
    receipts.len() == expected
        && receipts.iter().enumerate().all(|(index, receipt)| {
            receipt.index == u32::try_from(index).unwrap_or(u32::MAX)
                && receipt.exit_code == 0
                && valid_sha256(&receipt.stdout_sha256)
                && valid_sha256(&receipt.stderr_sha256)
        })
}

fn valid_checkpoint(reference: &str, sha256: &str) -> bool {
    valid_sha256(sha256)
        && reference
            .strip_prefix("home-")
            .and_then(|value| value.strip_suffix(".cma"))
            == Some(sha256)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_git_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
#[path = "control_correlation_tests.rs"]
mod tests;
