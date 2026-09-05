use super::state::{
    fingerprint, refused, remember_operation, replay_operation, runtime_home, safe_name,
    successful_commands, valid_sha256,
};
use super::MemoryRuntime;
use crate::wire::{
    AgentCloseReceipt, AgentHomeReceipt, AgentHomeRemoveReceipt, ResponseFrame, WireOperation,
};
use crate::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, SetupReceipt, WorkspaceReceipt,
    WorkspaceRemoveReceipt, WorkspaceRemoveRequest, WorkspaceRequest, WorkspaceSetupRequest,
    WorkspaceSource, WorktreeReceipt, WorktreeRef, WorktreeRemoveReceipt, WorktreeRequest,
};

pub(super) fn prepare_workspace(
    runtime: &mut MemoryRuntime,
    request: WorkspaceRequest,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::PrepareWorkspace;
    validate(
        request.has_valid_sha256(),
        !request.operation_id.trim().is_empty(),
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
    if !safe_name(&request.name) || runtime.workspaces.contains_key(&request.name) {
        return refused(operation, "invalid_or_existing_workspace");
    }
    let source = match &request.source {
        WorkspaceSource::Clone { url, reference } => {
            format!("{url}:{}", reference.as_deref().unwrap_or("default"))
        }
        WorkspaceSource::Init => format!("init:{}", request.name),
    };
    let receipt = WorkspaceReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace_id: request.workspace_id,
        name: request.name.clone(),
        path: format!("{}/{}", runtime_home(runtime), request.name),
        resolved_commit: super::state::digest(source.as_bytes()),
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

pub(super) fn setup_workspace(
    runtime: &mut MemoryRuntime,
    request: WorkspaceSetupRequest,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::WorkspaceSetup;
    validate(
        request.has_valid_sha256(),
        !request.operation_id.trim().is_empty(),
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

pub(super) fn remove_workspace(
    runtime: &mut MemoryRuntime,
    request: WorkspaceRemoveRequest,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::RemoveWorkspace;
    validate(
        request.has_valid_sha256(),
        !request.operation_id.trim().is_empty(),
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
    let Some(workspace) = runtime.workspaces.get(&request.name) else {
        return refused(operation, "workspace_not_found");
    };
    if workspace.workspace_id != request.workspace_id {
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

pub(super) fn add_worktree(
    runtime: &mut MemoryRuntime,
    request: WorktreeRequest,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::AddWorktree;
    let expected = format!("worktree:{}:{}:add:v1", request.workspace, request.name);
    validate(
        request.has_valid_sha256(),
        request.operation_id == expected,
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
    if !safe_name(&request.name)
        || !runtime.workspaces.contains_key(&request.workspace)
        || runtime
            .worktrees
            .contains_key(&(request.workspace.clone(), request.name.clone()))
    {
        return refused(operation, "invalid_or_existing_worktree");
    }
    let receipt = WorktreeReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace: request.workspace.clone(),
        name: request.name.clone(),
        path: format!(
            "{}/{}/.worktrees/{}",
            runtime_home(runtime),
            request.workspace,
            request.name
        ),
        branch: format!("cma/{}", request.name),
        base_commit: request.base,
    };
    runtime
        .worktrees
        .insert((request.workspace, request.name), receipt.clone());
    finish(
        runtime,
        request.operation_id,
        request.request_sha256,
        payload,
        ResponseFrame::AddWorktree(receipt),
    )
}

pub(super) fn remove_worktree(
    runtime: &mut MemoryRuntime,
    request: WorktreeRef,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::RemoveWorktree;
    let expected = format!("worktree:{}:{}:remove:v1", request.workspace, request.name);
    validate(
        request.has_valid_sha256(),
        request.operation_id == expected
            && safe_identity(&request.workspace)
            && safe_identity(&request.name),
        operation,
    )?;
    let payload = fingerprint(&request)?;
    let path = format!(
        "{}/{}/.worktrees/{}",
        runtime_home(runtime),
        request.workspace,
        request.name
    );
    if runtime
        .agents
        .values()
        .any(|agent| agent.running && agent.launch.cwd == path)
    {
        return refused(operation, "worktree_agent_open");
    }
    if let Some(response) = replay_operation(
        runtime,
        &request.operation_id,
        &request.request_sha256,
        &payload,
        operation,
    )? {
        return Ok(response);
    }
    let key = (request.workspace.clone(), request.name.clone());
    runtime.worktrees.remove(&key);
    let response = ResponseFrame::RemoveWorktree(WorktreeRemoveReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace: request.workspace,
        name: request.name,
    });
    finish(
        runtime,
        request.operation_id,
        request.request_sha256,
        payload,
        response,
    )
}

pub(super) fn materialize_home(
    runtime: &mut MemoryRuntime,
    plan: AgentHomePlan,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::MaterializeAgentHome;
    let expected = format!("agent:{}:home:create:v1", plan.agent_id.as_str());
    validate(
        valid_sha256(&plan.request_sha256),
        plan.operation_id == expected,
        operation,
    )?;
    let payload = plan.payload_sha256();
    if let Some(response) = replay_operation(
        runtime,
        &plan.operation_id,
        &plan.request_sha256,
        &payload,
        operation,
    )? {
        return Ok(response);
    }
    if plan.agent_id.0.is_empty() || runtime.homes.contains_key(&plan.agent_id) {
        return refused(operation, "invalid_or_existing_agent_home");
    }
    let receipt = AgentHomeReceipt {
        operation_id: plan.operation_id.clone(),
        request_sha256: plan.request_sha256.clone(),
        agent_id: plan.agent_id.clone(),
        path: format!(
            "{}/.cma/agents/{}/home",
            runtime_home(runtime),
            plan.agent_id.as_str()
        ),
    };
    runtime.homes.insert(plan.agent_id.clone(), plan);
    finish(
        runtime,
        receipt.operation_id.clone(),
        receipt.request_sha256.clone(),
        payload,
        ResponseFrame::MaterializeAgentHome(receipt),
    )
}

pub(super) fn close_agent(
    runtime: &mut MemoryRuntime,
    request: AgentCloseRequest,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::CloseAgent;
    let expected = format!("agent:{}:close:v1", request.agent_id.as_str());
    validate(
        request.has_valid_sha256(),
        request.operation_id == expected && safe_identity(request.agent_id.as_str()),
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
    if let Some(agent) = runtime.agents.get_mut(&request.agent_id) {
        agent.running = false;
    }
    let response = ResponseFrame::CloseAgent(AgentCloseReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        agent_id: request.agent_id,
    });
    finish(
        runtime,
        request.operation_id,
        request.request_sha256,
        payload,
        response,
    )
}

pub(super) fn remove_home(
    runtime: &mut MemoryRuntime,
    request: AgentHomeRemoveRequest,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::RemoveAgentHome;
    let expected = format!("agent:{}:home:remove:v1", request.agent_id.as_str());
    validate(
        request.has_valid_sha256(),
        request.operation_id == expected && safe_identity(request.agent_id.as_str()),
        operation,
    )?;
    let payload = fingerprint(&request)?;
    if runtime
        .agents
        .get(&request.agent_id)
        .is_some_and(|agent| agent.running)
    {
        return refused(operation, "agent_running");
    }
    if let Some(response) = replay_operation(
        runtime,
        &request.operation_id,
        &request.request_sha256,
        &payload,
        operation,
    )? {
        return Ok(response);
    }
    runtime.homes.remove(&request.agent_id);
    let response = ResponseFrame::RemoveAgentHome(AgentHomeRemoveReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        agent_id: request.agent_id.clone(),
    });
    finish(
        runtime,
        request.operation_id,
        request.request_sha256,
        payload,
        response,
    )
}

fn finish(
    runtime: &mut MemoryRuntime,
    operation_id: String,
    request_sha256: String,
    payload_sha256: String,
    response: ResponseFrame,
) -> crate::Result<ResponseFrame> {
    remember_operation(
        runtime,
        operation_id,
        request_sha256,
        payload_sha256,
        response.clone(),
    );
    Ok(response)
}

fn validate(
    digest_valid: bool,
    identity_valid: bool,
    operation: WireOperation,
) -> crate::Result<()> {
    if digest_valid && identity_valid {
        Ok(())
    } else {
        refused(operation, "operation_request_invalid")
    }
}

fn safe_identity(value: &str) -> bool {
    value.len() <= 64 && !value.starts_with('.') && safe_name(value)
}
