use std::path::Path;

use super::{corrupt, Record};
use crate::failure::Result;

pub(super) fn validate_request_and_receipt(
    record: &Record,
    home: &Path,
    allow_local_git: bool,
) -> Result<()> {
    let valid = match record.kind.as_str() {
        "prepare" => {
            serde_json::from_value::<layer3_adapter::WorkspaceRequest>(record.request.clone())
                .is_ok_and(|request| {
                    valid_prepare_request(&request, record, allow_local_git)
                        && receipt_is_absent_or(record, |value| {
                            serde_json::from_value::<layer3_adapter::WorkspaceReceipt>(
                                value.clone(),
                            )
                            .is_ok_and(|receipt| valid_workspace_receipt(&request, &receipt, home))
                        })
                })
        }
        "setup" => {
            serde_json::from_value::<layer3_adapter::WorkspaceSetupRequest>(record.request.clone())
                .is_ok_and(|request| {
                    valid_setup_request(&request, record)
                        && receipt_is_absent_or(record, |value| {
                            serde_json::from_value::<layer3_adapter::SetupReceipt>(value.clone())
                                .is_ok_and(|receipt| valid_setup_receipt(&request, &receipt))
                        })
                })
        }
        "remove" => {
            serde_json::from_value::<layer3_adapter::WorkspaceRemoveRequest>(record.request.clone())
                .is_ok_and(|request| {
                    valid_remove_request(&request, record)
                        && receipt_is_absent_or(record, |value| {
                            serde_json::from_value::<layer3_adapter::WorkspaceRemoveReceipt>(
                                value.clone(),
                            )
                            .is_ok_and(|receipt| valid_remove_receipt(&request, &receipt))
                        })
                })
        }
        _ => false,
    };
    valid
        .then_some(())
        .ok_or_else(|| corrupt("workspace journal request or receipt is invalid"))
}

fn valid_prepare_request(
    request: &layer3_adapter::WorkspaceRequest,
    record: &Record,
    allow_local_git: bool,
) -> bool {
    request_identity_matches(
        request.workspace_id.as_str(),
        &request.operation_id,
        &request.request_sha256,
        request.has_valid_sha256(),
        record,
    ) && crate::paths::safe_component(&request.name, "workspace name").is_ok()
        && crate::workspace_source::validate_clone_depth(request.clone_depth).is_ok()
        && match &request.source {
            layer3_adapter::WorkspaceSource::Clone { url, reference } => {
                crate::workspace_source::validate_clone_url(url, allow_local_git).is_ok()
                    && reference.as_deref().is_none_or(|reference| {
                        crate::paths::validate_selector(reference, "clone reference").is_ok()
                    })
            }
            layer3_adapter::WorkspaceSource::RepositorySnapshot { url, reference } => {
                crate::workspace_source::validate_clone_url(url, false).is_ok()
                    && reference.as_deref().is_none_or(|reference| {
                        crate::paths::validate_selector(reference, "repository ref").is_ok()
                    })
            }
            layer3_adapter::WorkspaceSource::PublicRepositorySnapshot { url, commit } => {
                layer3_adapter::public_repository_archive_url(url, commit).is_some()
            }
            layer3_adapter::WorkspaceSource::Init => true,
        }
}

fn valid_setup_request(request: &layer3_adapter::WorkspaceSetupRequest, record: &Record) -> bool {
    request_identity_matches(
        request.workspace_id.as_str(),
        &request.operation_id,
        &request.request_sha256,
        request.has_valid_sha256(),
        record,
    ) && crate::paths::safe_component(&request.name, "workspace name").is_ok()
        && crate::command::CommandLimits::install(
            request.timeout_seconds,
            request.total_timeout_seconds,
            request.output_limit_bytes,
        )
        .is_ok()
        && crate::command::validate_commands(&request.commands).is_ok()
}

fn valid_remove_request(request: &layer3_adapter::WorkspaceRemoveRequest, record: &Record) -> bool {
    request_identity_matches(
        request.workspace_id.as_str(),
        &request.operation_id,
        &request.request_sha256,
        request.has_valid_sha256(),
        record,
    ) && crate::paths::safe_component(&request.name, "workspace name").is_ok()
}

fn request_identity_matches(
    workspace_id: &str,
    operation_id: &str,
    request_sha256: &str,
    digest_valid: bool,
    record: &Record,
) -> bool {
    workspace_id == record.workspace_id
        && operation_id == record.operation_id
        && request_sha256 == record.request_sha256
        && digest_valid
}

fn receipt_is_absent_or(
    record: &Record,
    validate: impl FnOnce(&serde_json::Value) -> bool,
) -> bool {
    record.receipt.as_ref().is_none_or(validate)
}

fn valid_workspace_receipt(
    request: &layer3_adapter::WorkspaceRequest,
    receipt: &layer3_adapter::WorkspaceReceipt,
    home: &Path,
) -> bool {
    receipt.operation_id == request.operation_id
        && receipt.request_sha256 == request.request_sha256
        && receipt.workspace_id == request.workspace_id
        && receipt.name == request.name
        && Path::new(&receipt.path) == home.join(&request.name)
        && valid_commit(&receipt.resolved_commit)
        && match request.source {
            layer3_adapter::WorkspaceSource::RepositorySnapshot { .. } => {
                receipt.source_commit.as_deref().is_some_and(valid_commit)
            }
            layer3_adapter::WorkspaceSource::PublicRepositorySnapshot { ref commit, .. } => {
                receipt.source_commit.as_ref() == Some(commit)
            }
            _ => receipt.source_commit.is_none(),
        }
}

fn valid_setup_receipt(
    request: &layer3_adapter::WorkspaceSetupRequest,
    receipt: &layer3_adapter::SetupReceipt,
) -> bool {
    receipt.operation_id == request.operation_id
        && receipt.request_sha256 == request.request_sha256
        && receipt.workspace_id == request.workspace_id
        && receipt.commands.len() == request.commands.len()
        && receipt.commands.iter().enumerate().all(|(index, receipt)| {
            receipt.index == u32::try_from(index).unwrap_or(u32::MAX)
                && receipt.exit_code == 0
                && lower_sha256(&receipt.stdout_sha256)
                && lower_sha256(&receipt.stderr_sha256)
        })
}

fn valid_remove_receipt(
    request: &layer3_adapter::WorkspaceRemoveRequest,
    receipt: &layer3_adapter::WorkspaceRemoveReceipt,
) -> bool {
    receipt.operation_id == request.operation_id
        && receipt.request_sha256 == request.request_sha256
        && receipt.workspace_id == request.workspace_id
}

fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(lower_hex)
}

pub(super) fn lower_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(lower_hex)
}

fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}
