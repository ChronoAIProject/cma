use super::mutations::{finish, validate};
use super::state::{
    fingerprint, refused, replay_operation, runtime_home, safe_identity, safe_name,
    successful_commands,
};
use super::MemoryRuntime;
use crate::wire::{ResponseFrame, WireOperation};
use crate::{
    SetupReceipt, WorkspaceReceipt, WorkspaceRemoveReceipt, WorkspaceRemoveRequest,
    WorkspaceRequest, WorkspaceSetupRequest, WorkspaceSource,
};

pub(super) fn prepare(
    runtime: &mut MemoryRuntime,
    request: WorkspaceRequest,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::PrepareWorkspace;
    validate(
        request.has_valid_sha256(),
        !request.operation_id.trim().is_empty(),
        operation,
    )?;
    require_present(runtime, &request.workspace_id, operation)?;
    let payload = fingerprint(&request)?;
    if let Some(response) = replay_operation(
        runtime,
        &request.operation_id,
        &request.request_sha256,
        &payload,
        operation,
    )? {
        return Ok(response);
    }
    if !safe_name(&request.name) || runtime.workspaces.contains_key(&request.name) {
        return refused(operation, "invalid_or_existing_workspace");
    }
    let source = match &request.source {
        WorkspaceSource::Clone { url, reference }
        | WorkspaceSource::RepositorySnapshot { url, reference } => {
            format!("{url}:{}", reference.as_deref().unwrap_or("default"))
        }
        WorkspaceSource::PublicRepositorySnapshot { url, commit } => format!("{url}:{commit}"),
        WorkspaceSource::Init => format!("init:{}", request.name),
    };
    let receipt = WorkspaceReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace_id: request.workspace_id,
        name: request.name.clone(),
        path: format!("{}/{}", runtime_home(runtime), request.name),
        resolved_commit: super::state::digest(source.as_bytes()),
        source_commit: match request.source {
            WorkspaceSource::RepositorySnapshot { .. } => Some("a".repeat(40)),
            WorkspaceSource::PublicRepositorySnapshot { commit, .. } => Some(commit),
            _ => None,
        },
    };
    runtime.workspaces.insert(request.name, receipt.clone());
    finish(
        runtime,
        request.operation_id,
        request.request_sha256,
        payload,
        ResponseFrame::PrepareWorkspace(receipt),
    )
}

pub(super) fn setup(
    runtime: &mut MemoryRuntime,
    request: WorkspaceSetupRequest,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::WorkspaceSetup;
    validate(
        request.has_valid_sha256(),
        !request.operation_id.trim().is_empty(),
        operation,
    )?;
    require_present(runtime, &request.workspace_id, operation)?;
    let payload = fingerprint(&request)?;
    if let Some(response) = replay_operation(
        runtime,
        &request.operation_id,
        &request.request_sha256,
        &payload,
        operation,
    )? {
        return Ok(response);
    }
    let Some(workspace) = runtime.workspaces.get(&request.name) else {
        return refused(operation, "workspace_not_found");
    };
    if workspace.workspace_id != request.workspace_id {
        return refused(operation, "workspace_identity_mismatch");
    }
    let response = ResponseFrame::WorkspaceSetup(SetupReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace_id: request.workspace_id,
        commands: successful_commands(&request.commands),
    });
    finish(
        runtime,
        request.operation_id,
        request.request_sha256,
        payload,
        response,
    )
}

pub(super) fn remove(
    runtime: &mut MemoryRuntime,
    request: WorkspaceRemoveRequest,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::RemoveWorkspace;
    validate(
        request.has_valid_sha256(),
        !request.operation_id.trim().is_empty()
            && safe_identity(&request.workspace_id)
            && safe_name(&request.name),
        operation,
    )?;
    let payload = fingerprint(&request)?;
    if let Some(response) = replay_operation(
        runtime,
        &request.operation_id,
        &request.request_sha256,
        &payload,
        operation,
    )? {
        return Ok(response);
    }
    if runtime
        .workspaces
        .get(&request.name)
        .is_some_and(|workspace| workspace.workspace_id != request.workspace_id)
    {
        return refused(operation, "workspace_identity_mismatch");
    }
    runtime.workspaces.remove(&request.name);
    runtime
        .worktrees
        .retain(|(workspace, _), _| workspace != &request.name);
    let response = ResponseFrame::RemoveWorkspace(WorkspaceRemoveReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace_id: request.workspace_id,
    });
    finish(
        runtime,
        request.operation_id,
        request.request_sha256,
        payload,
        response,
    )
}

fn require_present(
    runtime: &MemoryRuntime,
    workspace_id: &str,
    operation: WireOperation,
) -> crate::Result<()> {
    if runtime.operations.values().any(|entry| matches!(&entry.response, ResponseFrame::RemoveWorkspace(receipt) if receipt.workspace_id == workspace_id)) {
        return refused(operation, "workspace_removed");
    }
    Ok(())
}
