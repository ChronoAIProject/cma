use super::state::{
    fingerprint, refused, remember_operation, replay_operation, runtime_home, safe_identity,
    safe_name, valid_sha256,
};
use super::{authority, MemoryRuntime};
use crate::wire::{
    AgentHomeReceipt, AgentHomeRemoveReceipt, FencedReceipt, FencedRequest, ResponseFrame,
    WireOperation,
};
use crate::{
    AgentHomePlan, AgentHomeRemoveRequest, WorktreeReceipt, WorktreeRef, WorktreeRemoveReceipt,
    WorktreeRequest,
};

#[cfg(test)]
#[path = "memory_skill_tests.rs"]
mod skill_tests;

pub(super) fn add_worktree(
    runtime: &mut MemoryRuntime,
    request: FencedRequest<WorktreeRequest>,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::AddWorktree;
    let authority_fence = request.authority_fence;
    let request = request.request;
    let expected = format!("worktree:{}:{}:add:v1", request.workspace, request.name);
    validate(
        request.has_valid_sha256(),
        request.operation_id == expected,
        operation,
    )?;
    let payload = fingerprint(&request)?;
    authority::admit(
        runtime,
        authority::ResourceKey::Worktree(request.workspace.clone(), request.name.clone()),
        authority::Disposition::Present,
        authority_fence,
        &request.operation_id,
        &request.request_sha256,
        &payload,
        operation,
    )?;
    if let Some(response) = replay_operation(
        runtime,
        &request.operation_id,
        &request.request_sha256,
        &payload,
        operation,
    )? {
        let ResponseFrame::AddWorktree(mut response) = response else {
            return refused(operation, "operation_replay_mismatch");
        };
        response.authority_fence = authority_fence;
        return Ok(ResponseFrame::AddWorktree(response));
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
        ResponseFrame::AddWorktree(FencedReceipt::new(authority_fence, receipt)),
    )
}

pub(super) fn remove_worktree(
    runtime: &mut MemoryRuntime,
    request: FencedRequest<WorktreeRef>,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::RemoveWorktree;
    let authority_fence = request.authority_fence;
    let request = request.request;
    let expected = format!("worktree:{}:{}:remove:v1", request.workspace, request.name);
    validate(
        request.has_valid_sha256(),
        request.operation_id == expected
            && safe_identity(&request.workspace)
            && safe_identity(&request.name),
        operation,
    )?;
    let payload = fingerprint(&request)?;
    authority::admit(
        runtime,
        authority::ResourceKey::Worktree(request.workspace.clone(), request.name.clone()),
        authority::Disposition::Absent,
        authority_fence,
        &request.operation_id,
        &request.request_sha256,
        &payload,
        operation,
    )?;
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
        let ResponseFrame::RemoveWorktree(mut response) = response else {
            return refused(operation, "operation_replay_mismatch");
        };
        response.authority_fence = authority_fence;
        return Ok(ResponseFrame::RemoveWorktree(response));
    }
    let key = (request.workspace.clone(), request.name.clone());
    runtime.worktrees.remove(&key);
    let response = ResponseFrame::RemoveWorktree(FencedReceipt::new(
        authority_fence,
        WorktreeRemoveReceipt {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            workspace: request.workspace,
            name: request.name,
        },
    ));
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
    plan: FencedRequest<AgentHomePlan>,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::MaterializeAgentHome;
    let authority_fence = plan.authority_fence;
    let plan = plan.request;
    let expected = format!("agent:{}:home:create:v1", plan.agent_id.as_str());
    validate(
        valid_sha256(&plan.request_sha256),
        plan.operation_id == expected,
        operation,
    )?;
    let payload = plan.payload_sha256();
    authority::admit(
        runtime,
        authority::ResourceKey::AgentHome(plan.agent_id.clone()),
        authority::Disposition::Present,
        authority_fence,
        &plan.operation_id,
        &plan.request_sha256,
        &payload,
        operation,
    )?;
    if let Some(response) = replay_operation(
        runtime,
        &plan.operation_id,
        &plan.request_sha256,
        &payload,
        operation,
    )? {
        let ResponseFrame::MaterializeAgentHome(mut response) = response else {
            return refused(operation, "operation_replay_mismatch");
        };
        response.authority_fence = authority_fence;
        return Ok(ResponseFrame::MaterializeAgentHome(response));
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
        ResponseFrame::MaterializeAgentHome(FencedReceipt::new(authority_fence, receipt)),
    )
}

pub(super) fn remove_home(
    runtime: &mut MemoryRuntime,
    request: FencedRequest<AgentHomeRemoveRequest>,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::RemoveAgentHome;
    let authority_fence = request.authority_fence;
    let request = request.request;
    let expected = format!("agent:{}:home:remove:v1", request.agent_id.as_str());
    validate(
        request.has_valid_sha256(),
        request.operation_id == expected && safe_identity(request.agent_id.as_str()),
        operation,
    )?;
    let payload = fingerprint(&request)?;
    authority::admit(
        runtime,
        authority::ResourceKey::AgentHome(request.agent_id.clone()),
        authority::Disposition::Absent,
        authority_fence,
        &request.operation_id,
        &request.request_sha256,
        &payload,
        operation,
    )?;
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
        let ResponseFrame::RemoveAgentHome(mut response) = response else {
            return refused(operation, "operation_replay_mismatch");
        };
        response.authority_fence = authority_fence;
        return Ok(ResponseFrame::RemoveAgentHome(response));
    }
    runtime.homes.remove(&request.agent_id);
    let response = ResponseFrame::RemoveAgentHome(FencedReceipt::new(
        authority_fence,
        AgentHomeRemoveReceipt {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            agent_id: request.agent_id.clone(),
        },
    ));
    finish(
        runtime,
        request.operation_id,
        request.request_sha256,
        payload,
        response,
    )
}

pub(super) fn finish(
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

pub(super) fn validate(
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

pub(super) fn install_skill(
    runtime: &mut MemoryRuntime,
    fenced: FencedRequest<crate::AgentSkillInstallRequest>,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::InstallAgentSkill;
    let fence = fenced.authority_fence;
    let request = fenced.request;
    validate(
        request.has_valid_sha256(),
        request.operation_id == request.expected_operation_id(),
        operation,
    )?;
    let payload = request.request_sha256.clone();
    authority::admit(
        runtime,
        authority::ResourceKey::AgentSkill(request.agent_id.clone(), request.skill.id.clone()),
        authority::Disposition::Present,
        fence,
        &request.operation_id,
        &request.request_sha256,
        &payload,
        operation,
    )?;
    if let Some(response) = replay_operation(
        runtime,
        &request.operation_id,
        &request.request_sha256,
        &payload,
        operation,
    )? {
        let ResponseFrame::InstallAgentSkill(mut response) = response else {
            return refused(operation, "operation_replay_mismatch");
        };
        response.authority_fence = fence;
        return Ok(ResponseFrame::InstallAgentSkill(response));
    }
    let Some(home) = runtime.homes.get_mut(&request.agent_id) else {
        return refused(operation, "agent_home_missing");
    };
    if !safe_name(&request.skill.id) {
        return refused(operation, "skill_destination_invalid");
    }
    match &request.upgrade {
        Some(upgrade) => {
            let Some(previous) = home.skills.iter_mut().find(|s| s.id == request.skill.id) else {
                return Err(crate::AdapterError::OperationIdentityConflict(
                    "agent_skill_upgrade_conflict: previous installation differs".into(),
                ));
            };
            // Initial home plans may omit display names; the runtime proves archive content.
            if previous.id != upgrade.previous.id
                || previous.version != upgrade.previous.version
                || previous.archive_sha256 != upgrade.previous.archive_sha256
            {
                return Err(crate::AdapterError::OperationIdentityConflict(
                    "agent_skill_upgrade_conflict: previous installation differs".into(),
                ));
            }
            *previous = request.skill.clone();
        }
        None => {
            if home.skills.iter().any(|s| s.id == request.skill.id) {
                return refused(operation, "skill_destination_exists");
            }
            home.skills.push(request.skill.clone());
        }
    }
    let receipt = crate::AgentSkillInstallReceipt::from_request(&request);
    finish(
        runtime,
        request.operation_id,
        request.request_sha256,
        payload,
        ResponseFrame::InstallAgentSkill(FencedReceipt::new(fence, receipt)),
    )
}
