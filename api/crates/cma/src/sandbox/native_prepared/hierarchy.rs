//! Stable runtime request identities survive transfer to the owned hierarchy.

use layer2_core::sandbox::{WarmSandbox, WarmSandboxState};
use layer3_adapter::{
    AgentHomeRequest, AgentOpenRequest, RuntimeHandle, SandboxAdapter, WorkspaceRequest,
    WorkspaceSetupRequest, WorkspaceSource, WorktreeRequest,
};

use super::rejected;

pub(super) async fn prepare_step(
    adapter: &dyn SandboxAdapter,
    record: &mut WarmSandbox,
    handle: &RuntimeHandle,
    proof: &mut Option<super::NativePreparedWorkspaceProof>,
) -> layer3_adapter::Result<()> {
    let execution = record.plan.execution;
    let workspace = &mut record.workspaces[0];
    if workspace.resolved_commit.is_none() {
        let request = WorkspaceRequest::with_clone_depth(
            format!("workspace:{}:prepare:v1", workspace.id),
            workspace.id.to_string(),
            "workspace",
            WorkspaceSource::Init,
            execution.clone_depth,
        );
        let receipt = adapter.prepare_workspace(handle, &request).await?;
        workspace.resolved_commit = Some(receipt.resolved_commit.clone());
        *proof = Some(
            super::NativePreparedWorkspaceProof::prepared(record, receipt)
                .map_err(|e| rejected(&e.to_string()))?,
        );
        return Ok(());
    }
    if !workspace.setup_ready {
        let request = WorkspaceSetupRequest::with_timeouts(
            format!("workspace:{}:setup:v1", workspace.id),
            workspace.id.to_string(),
            "workspace",
            Vec::new(),
            execution.command_timeout_seconds,
            execution.total_timeout_seconds,
            layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        );
        let receipt = adapter.run_workspace_setup(handle, &request).await?;
        if receipt.operation_id != request.operation_id
            || receipt.request_sha256 != request.request_sha256
            || receipt.workspace_id != request.workspace_id
            || !receipt.commands.is_empty()
        {
            return Err(rejected("prepared workspace setup receipt is invalid"));
        }
        workspace.setup_ready = true;
        // Legacy partial preparations lack the actual Prepare receipt: never infer it.
        if let Some(prepared) = proof.as_ref() {
            *proof = Some(
                prepared
                    .completed(record, receipt)
                    .map_err(|e| rejected(&e.to_string()))?,
            );
        }
        return Ok(());
    }
    let agent = &mut workspace.agents[0];
    if agent.skills.is_none() {
        agent.skills = Some(Vec::new());
        return Ok(());
    }
    if agent.conversation_id.is_none() {
        let request = WorktreeRequest::new(
            "worktree:workspace:wt-1:add:v1",
            "workspace",
            "wt-1",
            workspace
                .resolved_commit
                .clone()
                .expect("validated workspace base"),
        );
        adapter.add_worktree(handle, 1, &request).await?;
        let options = super::super::runtime_mapping::profile_runtime_options(
            &record.plan.workspaces[0].agents[0].content,
            &record.plan.profile.content,
            &agent.id,
            "/home/cma/workspace/.worktrees/wt-1",
        );
        let home = AgentHomeRequest::new(
            format!("agent:{}:home:create:v1", agent.id),
            options.clone(),
            String::new(),
            Vec::new(),
        );
        adapter.materialize_agent_home(handle, 1, &home).await?;
        let request = AgentOpenRequest::new(format!("agent:{}:open:v1", agent.id), options, None);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(256);
        // JoinSet also cancels the drain if the outer leased work is cancelled.
        let mut drain = tokio::task::JoinSet::new();
        drain.spawn(async move { while receiver.recv().await.is_some() {} });
        let opened = adapter.open_agent(handle, 1, &request, sender).await;
        drain.shutdown().await;
        agent.conversation_id = Some(
            opened?
                .conversation_id
                .ok_or_else(|| rejected("prepared Agent conversation missing"))?,
        );
        return Ok(());
    }
    if workspace.is_ready() {
        record.state = WarmSandboxState::Ready;
    }
    Ok(())
}

pub(super) async fn verify_session(
    adapter: &dyn SandboxAdapter,
    record: &WarmSandbox,
    handle: &RuntimeHandle,
) -> layer3_adapter::Result<()> {
    let agent = &record.workspaces[0].agents[0];
    let conversation = agent
        .conversation_id
        .as_deref()
        .ok_or_else(|| rejected("prepared conversation missing"))?;
    let options = super::super::runtime_mapping::profile_runtime_options(
        &record.plan.workspaces[0].agents[0].content,
        &record.plan.profile.content,
        &agent.id,
        "/home/cma/workspace/.worktrees/wt-1",
    );
    let request = AgentOpenRequest::new(format!("agent:{}:open:v1", agent.id), options, None);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(256);
    let mut drain = tokio::task::JoinSet::new();
    drain.spawn(async move { while receiver.recv().await.is_some() {} });
    let opened = adapter
        .reconnect_agent(handle, 1, &request, conversation, sender)
        .await;
    drain.shutdown().await;
    if opened?.conversation_id.as_deref() != Some(conversation) {
        return Err(rejected("prepared conversation identity changed"));
    }
    Ok(())
}
