use super::state::{fingerprint, refused, replay_operation, safe_identity, valid_sha256};
use super::{authority, mutations::validate, MemoryRuntime};
use crate::wire::{AgentCloseReceipt, FencedReceipt, FencedRequest, ResponseFrame, WireOperation};
use crate::AgentProcessAction::{Close, Open};
use crate::{AgentCloseRequest, AgentLaunch};

pub(super) fn authorize_open(
    runtime: &MemoryRuntime,
    request: &FencedRequest<AgentLaunch>,
) -> crate::Result<u64> {
    let launch = &request.request;
    let agent = launch.agent_id.as_str();
    let Some(incarnation) = Open.incarnation(agent, &launch.operation_id) else {
        return refused(WireOperation::OpenAgent, "operation_request_invalid");
    };
    if !valid_sha256(&launch.request_sha256) || request.authority_fence == 0 {
        return refused(WireOperation::OpenAgent, "operation_request_invalid");
    }
    if incarnation == 1 {
        if request.restart_runtime.is_some() {
            return refused(
                WireOperation::OpenAgent,
                "agent_process_restart_runtime_unexpected",
            );
        }
        return Ok(incarnation);
    }
    let Some(writer) = &request.restart_runtime else {
        return refused(
            WireOperation::OpenAgent,
            "agent_process_restart_runtime_missing",
        );
    };
    let ResponseFrame::RuntimeStatus(status) =
        super::status::inspect(runtime, writer.runtime.clone())?
    else {
        return refused(
            WireOperation::OpenAgent,
            "agent_process_restart_runtime_changed",
        );
    };
    if status.boot_id != writer.boot_id {
        return refused(
            WireOperation::OpenAgent,
            "agent_process_restart_runtime_changed",
        );
    }
    if runtime.operations.contains_key(&launch.operation_id) {
        return Ok(incarnation);
    }
    let predecessor = Open.operation_id(agent, incarnation - 1);
    let close = Close.operation_id(agent, incarnation - 1);
    let previous_open = runtime.operations.get(&predecessor).map(|op| &op.response);
    let previous_close = runtime.operations.get(&close).map(|op| &op.response);
    if !matches!(previous_close, Some(ResponseFrame::CloseAgent(_)))
        || !matches!(previous_open, Some(ResponseFrame::OpenAgent(open)) if open.receipt.cwd == launch.cwd && open.receipt.home == launch.home)
    {
        return refused(
            WireOperation::OpenAgent,
            "agent_process_predecessor_unclosed",
        );
    }
    if authority::completed_close_fence(runtime, &launch.agent_id, incarnation - 1, &close)
        .is_none_or(|fence| fence >= request.authority_fence)
    {
        return refused(WireOperation::OpenAgent, "mutation_authority_stale");
    }
    Ok(incarnation)
}

pub(super) fn close_agent(
    runtime: &mut MemoryRuntime,
    request: FencedRequest<AgentCloseRequest>,
) -> crate::Result<ResponseFrame> {
    let operation = WireOperation::CloseAgent;
    let authority_fence = request.authority_fence;
    let request = request.request;
    let Some(incarnation) = Close.incarnation(request.agent_id.as_str(), &request.operation_id)
    else {
        return refused(operation, "operation_request_invalid");
    };
    validate(
        request.has_valid_sha256(),
        safe_identity(request.agent_id.as_str()),
        operation,
    )?;
    if incarnation > 1 {
        let open = Open.operation_id(request.agent_id.as_str(), incarnation);
        if !matches!(
            runtime.operations.get(&open).map(|op| &op.response),
            Some(ResponseFrame::OpenAgent(_))
        ) {
            return refused(operation, "agent_process_incarnation_unknown");
        }
    }
    let payload = fingerprint(&request)?;
    authority::admit(
        runtime,
        authority::ResourceKey::AgentProcess(request.agent_id.clone(), incarnation),
        authority::Disposition::Absent,
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
        let ResponseFrame::CloseAgent(mut response) = response else {
            return refused(operation, "operation_replay_mismatch");
        };
        response.authority_fence = authority_fence;
        return Ok(ResponseFrame::CloseAgent(response));
    }
    if let Some(agent) = runtime.agents.get_mut(&request.agent_id) {
        if agent.running
            && Open.incarnation(request.agent_id.as_str(), &agent.launch.operation_id)
                != Some(incarnation)
        {
            return refused(operation, "agent_process_incarnation_changed");
        }
        agent.running = false;
    }
    let response = ResponseFrame::CloseAgent(FencedReceipt::new(
        authority_fence,
        AgentCloseReceipt {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            agent_id: request.agent_id,
        },
    ));
    super::mutations::finish(
        runtime,
        request.operation_id,
        request.request_sha256,
        payload,
        response,
    )
}
