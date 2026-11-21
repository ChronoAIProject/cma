//! Combine transport only; preserve both original resource operation identities.
use std::path::Path;

use super::HostedAdapter;
use crate::wire::{AgentResourcesPlan, FencedRequest, RequestFrame, ResponseFrame};
use crate::{AdapterError, AgentHomeRequest, RuntimeHandle, WorktreeReceipt, WorktreeRequest};

pub(super) async fn prepare(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    fence: u64,
    worktree: &WorktreeRequest,
    home: &AgentHomeRequest,
) -> crate::Result<WorktreeReceipt> {
    let state = adapter.known_state(handle).await?;
    adapter.validate_options(handle, &home.options).await?;
    let expected_path = Path::new(&state.home_directory)
        .join(&worktree.workspace)
        .join(".worktrees")
        .join(&worktree.name);
    if fence == 0
        || !worktree.has_valid_sha256()
        || !home.has_valid_sha256()
        || worktree.operation_id
            != format!("worktree:{}:{}:add:v1", worktree.workspace, worktree.name)
        || home.operation_id != format!("agent:{}:home:create:v1", home.options.slot.as_str())
        || worktree.operation_id.len() > 256
        || home.operation_id.len() > 256
        || Path::new(&home.options.working_directory) != expected_path
    {
        return Err(AdapterError::Rejected(
            "Agent resource preparation changed operation identity or Worktree".into(),
        ));
    }
    let plan = adapter.runtime.home_plan(home)?;
    if plan.operation_id != home.operation_id
        || plan.request_sha256 != home.request_sha256
        || plan.agent_id != home.options.slot
    {
        return Err(AdapterError::Rejected(
            "Agent resource preparation changed the rendered home".into(),
        ));
    }
    let response = adapter
        .exchange(
            handle,
            RequestFrame::PrepareAgentResources(FencedRequest::new(
                fence,
                AgentResourcesPlan {
                    worktree: worktree.clone(),
                    home: plan,
                },
            )),
        )
        .await?;
    let ResponseFrame::PrepareAgentResources(response) = response else {
        return Err(AdapterError::unexpected_reply("prepare Agent resources"));
    };
    let receipt = response.receipt;
    if response.authority_fence != fence
        || receipt.worktree.operation_id != worktree.operation_id
        || receipt.worktree.request_sha256 != worktree.request_sha256
        || receipt.worktree.workspace != worktree.workspace
        || receipt.worktree.name != worktree.name
        || Path::new(&receipt.worktree.path) != expected_path
        || receipt.worktree.branch != format!("cma/{}", worktree.name)
        || !super::super::reply::valid_git_commit(&receipt.worktree.base_commit)
        || receipt.home.operation_id != home.operation_id
        || receipt.home.request_sha256 != home.request_sha256
        || receipt.home.agent_id != home.options.slot
        || Path::new(&receipt.home.path) != Path::new(&home.options.home_directory)
    {
        return Err(AdapterError::Rejected(
            "runtime returned mismatched Agent resource receipts".into(),
        ));
    }
    Ok(receipt.worktree)
}
