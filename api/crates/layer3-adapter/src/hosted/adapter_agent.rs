use std::path::Path;

use tokio::sync::mpsc;

use super::super::reply::{valid_git_commit, valid_sha256};
use super::HostedAdapter;
use crate::wire::{RequestFrame, ResponseFrame};
use crate::{
    AdapterError, AgentCloseRequest, AgentHomeRemoveRequest, AgentHomeRequest, AgentOpenRequest,
    ConversationChannel, ConversationUpdate, RuntimeHandle, WorktreeReceipt, WorktreeRef,
    WorktreeRequest,
};

pub(super) async fn add_worktree(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    request: &WorktreeRequest,
) -> crate::Result<WorktreeReceipt> {
    let state = adapter.known_state(handle).await?;
    validate_request(
        request,
        format!("worktree:{}:{}:add:v1", request.workspace, request.name),
    )?;
    match adapter
        .exchange(handle, RequestFrame::AddWorktree(request.clone()))
        .await?
    {
        ResponseFrame::AddWorktree(receipt)
            if receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && receipt.workspace == request.workspace
                && receipt.name == request.name
                && Path::new(&receipt.path)
                    == Path::new(&state.home_directory)
                        .join(&request.workspace)
                        .join(".worktrees")
                        .join(&request.name)
                && receipt.branch == format!("cma/{}", request.name)
                && valid_git_commit(&receipt.base_commit) =>
        {
            Ok(receipt)
        }
        ResponseFrame::AddWorktree(_) => Err(AdapterError::Rejected(
            "runtime returned a receipt for another worktree addition".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

pub(super) async fn remove_worktree(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    request: &WorktreeRef,
) -> crate::Result<()> {
    adapter.known_state(handle).await?;
    validate_request(
        request,
        format!("worktree:{}:{}:remove:v1", request.workspace, request.name),
    )?;
    match adapter
        .exchange(handle, RequestFrame::RemoveWorktree(request.clone()))
        .await?
    {
        ResponseFrame::RemoveWorktree(receipt)
            if receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && receipt.workspace == request.workspace
                && receipt.name == request.name =>
        {
            Ok(())
        }
        ResponseFrame::RemoveWorktree(_) => Err(AdapterError::Rejected(
            "runtime returned a receipt for another worktree removal".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

pub(super) async fn materialize_home(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    request: &AgentHomeRequest,
) -> crate::Result<()> {
    adapter.validate_options(handle, &request.options).await?;
    validate_request(
        request,
        format!("agent:{}:home:create:v1", request.options.slot.as_str()),
    )?;
    let plan = adapter.runtime.home_plan(request)?;
    if plan.operation_id != request.operation_id
        || plan.request_sha256 != request.request_sha256
        || plan.agent_id != request.options.slot
    {
        return Err(AdapterError::Rejected(
            "agent home plan changed the requested operation or slot".into(),
        ));
    }
    match adapter
        .exchange(handle, RequestFrame::MaterializeAgentHome(plan))
        .await?
    {
        ResponseFrame::MaterializeAgentHome(receipt)
            if receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && receipt.agent_id == request.options.slot
                && Path::new(&receipt.path) == Path::new(&request.options.home_directory) =>
        {
            Ok(())
        }
        ResponseFrame::MaterializeAgentHome(_) => Err(AdapterError::Rejected(
            "runtime returned a receipt for another agent home".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

pub(super) async fn remove_home(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    request: &AgentHomeRemoveRequest,
) -> crate::Result<()> {
    adapter.known_state(handle).await?;
    validate_request(
        request,
        format!("agent:{}:home:remove:v1", request.agent_id.as_str()),
    )?;
    match adapter
        .exchange(handle, RequestFrame::RemoveAgentHome(request.clone()))
        .await?
    {
        ResponseFrame::RemoveAgentHome(receipt)
            if receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && receipt.agent_id == request.agent_id =>
        {
            Ok(())
        }
        ResponseFrame::RemoveAgentHome(_) => Err(AdapterError::Rejected(
            "runtime returned a receipt for another agent home removal".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

pub(super) async fn open_agent(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    request: &AgentOpenRequest,
    updates: mpsc::Sender<ConversationUpdate>,
) -> crate::Result<ConversationChannel> {
    adapter.validate_options(handle, &request.options).await?;
    validate_request(
        request,
        format!("agent:{}:open:v1", request.options.slot.as_str()),
    )?;
    let launch = adapter.runtime.launch(request)?;
    if launch.operation_id != request.operation_id
        || launch.request_sha256 != request.request_sha256
        || launch.agent_id != request.options.slot
        || launch.cwd != request.options.working_directory
        || launch.home != request.options.home_directory
    {
        return Err(AdapterError::Rejected(
            "agent launch changed its operation, slot, working directory, or home".into(),
        ));
    }
    let (channel, receipt) = adapter
        .backend
        .open_agent(handle, launch, request.resume_conversation.clone(), updates)
        .await?;
    if receipt.operation_id != request.operation_id
        || receipt.request_sha256 != request.request_sha256
        || receipt.agent_id != request.options.slot
        || receipt.cwd != request.options.working_directory
        || receipt.home != request.options.home_directory
        || receipt.conversation_id != channel.conversation_id
    {
        return Err(AdapterError::Rejected(
            "runtime returned a receipt for another agent open operation".into(),
        ));
    }
    Ok(channel)
}

pub(super) async fn close_agent(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    request: &AgentCloseRequest,
) -> crate::Result<()> {
    adapter.known_state(handle).await?;
    validate_request(
        request,
        format!("agent:{}:close:v1", request.agent_id.as_str()),
    )?;
    match adapter
        .exchange(handle, RequestFrame::CloseAgent(request.clone()))
        .await?
    {
        ResponseFrame::CloseAgent(receipt)
            if receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && receipt.agent_id == request.agent_id =>
        {
            Ok(())
        }
        ResponseFrame::CloseAgent(_) => Err(AdapterError::Rejected(
            "runtime returned a receipt for another agent close operation".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

trait ReplayRequest {
    fn operation_id(&self) -> &str;
    fn request_sha256(&self) -> &str;
    fn digest_matches(&self) -> bool;
}

macro_rules! replay_request {
    ($($type:ty),+ $(,)?) => {
        $(impl ReplayRequest for $type {
            fn operation_id(&self) -> &str { &self.operation_id }
            fn request_sha256(&self) -> &str { &self.request_sha256 }
            fn digest_matches(&self) -> bool { self.has_valid_sha256() }
        })+
    };
}

replay_request!(
    WorktreeRequest,
    WorktreeRef,
    AgentHomeRequest,
    AgentHomeRemoveRequest,
    AgentOpenRequest,
    AgentCloseRequest,
);

fn validate_request(request: &impl ReplayRequest, expected: String) -> crate::Result<()> {
    if request.operation_id() != expected
        || request.operation_id().len() > 256
        || !valid_sha256(request.request_sha256())
        || !request.digest_matches()
    {
        return Err(AdapterError::Rejected(
            "operation id or request digest is invalid".into(),
        ));
    }
    Ok(())
}
