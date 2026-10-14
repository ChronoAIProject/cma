//! Exact consumed preparation authority for one completed Open and Ready publication.
use layer2_core::sandbox::{
    Agent, AgentOrigin, AgentStatus, AgentWorkStep, Sandbox, SandboxStatus, SandboxStoreError,
    Workspace, WorkspaceOrigin, WorkspaceStatus, Worktree, WorktreeStatus,
};
use layer3_adapter::{AgentHomeRequest, AgentOpenRequest, WorktreeRequest};
use mongodb::ClientSession;

use crate::store::sandbox_mongo::MongoSandboxStore;

pub(in crate::store) async fn open_ready_proof_revision(
    store: &MongoSandboxStore,
    agent: &Agent,
    sandbox: &Sandbox,
    workspace: &Workspace,
    tree: &Worktree,
    session: &mut ClientSession,
) -> Result<Option<u64>, SandboxStoreError> {
    if agent.status != AgentStatus::Starting
        || agent.work.step != AgentWorkStep::OpenConversation
        || agent.ready_at.is_some()
        || agent.deleted_at.is_some()
        || agent.requested_stop_reason.is_some()
        || agent.conversation.conversation_id.is_some()
        || agent.conversation.runtime_generation.is_some()
        || agent.conversation.active_turn.is_some()
        || agent.conversation.operation.is_some()
        || agent.work.receipts.len() != 2
        || agent.conversation.settings.is_some()
        || !agent.work.live_skills.is_empty()
        || agent.work.resolved_skills.as_deref() != Some(&[])
        || !agent.profile.content.skills.is_empty()
        || !agent.profile.content.system_prompt.is_empty()
        || sandbox.status != SandboxStatus::Active
        || !sandbox.credential.delivered
        || sandbox.lifecycle.checkpoint_capture.is_some()
        || sandbox.lifecycle.operation.is_some()
        || sandbox
            .runtime
            .as_ref()
            .is_none_or(|runtime| runtime.boot_id.is_none())
        || workspace.status != WorkspaceStatus::Ready
        || tree.status != WorktreeStatus::Creating
    {
        return Ok(None);
    }
    let Some(record) = super::adoption::claimed(store, sandbox, session).await? else {
        return Ok(None);
    };
    let owned = record
        .ownership
        .as_ref()
        .expect("claimed ownership checked");
    let prepared_workspace = &record.payload.workspaces[0];
    let prepared = &prepared_workspace.agents[0];
    let mut reaches = sandbox.credential.reaches.clone();
    reaches.sort();
    reaches.dedup();
    if owned.agent.as_ref() != Some(&agent.profile)
        || agent.owner != owned.owner
        || agent.sandbox_id != sandbox.id
        || agent.workspace_id != workspace.id
        || agent.worktree_id != tree.id
        || agent.id != prepared.id
        || agent.worktree_id != prepared.worktree_id
        || agent.work.warm_conversation_id != prepared.conversation_id
        || agent.work.warm_conversation_id.is_none()
        || agent.work.resolved_skills != prepared.skills
        || agent.origin
            != (AgentOrigin::Lifecycle {
                operation_id: owned.operation_id.clone(),
            })
        || sandbox.work.execution != Some(record.payload.plan.execution)
        || reaches != record.payload.plan.reaches
        || workspace.id != prepared_workspace.id
        || workspace.owner != owned.owner
        || workspace.sandbox_id != sandbox.id
        || workspace.profile != owned.workspace
        || workspace.origin != (WorkspaceOrigin::SandboxProfile { index: 0 })
        || workspace.name != "workspace"
        || workspace.path != "/home/cma/workspace"
        || workspace.resolved_commit != prepared_workspace.resolved_commit
        || !prepared_workspace.is_ready()
        || tree.owner != owned.owner
        || tree.sandbox_id != sandbox.id
        || tree.workspace_id != workspace.id
        || tree.agent_id.as_ref() != Some(&agent.id)
        || tree.name != "wt-1"
        || tree.path != "/home/cma/workspace/.worktrees/wt-1"
        || tree.branch != "cma/wt-1"
        || workspace.resolved_commit.as_ref() != Some(&tree.base_commit)
    {
        return Ok(None);
    }
    let worktree = WorktreeRequest::new(
        "worktree:workspace:wt-1:add:v1",
        "workspace",
        "wt-1",
        tree.base_commit.clone(),
    );
    let options = crate::sandbox::runtime_options(agent, sandbox, tree);
    let home = AgentHomeRequest::new(
        format!("agent:{}:home:create:v1", agent.id),
        options.clone(),
        String::new(),
        Vec::new(),
    );
    let open = AgentOpenRequest::new(format!("agent:{}:open:v1", agent.id), options, None);
    let expected = [
        (
            AgentWorkStep::AddWorktree,
            worktree.operation_id,
            worktree.request_sha256,
        ),
        (
            AgentWorkStep::MaterializeHome,
            home.operation_id,
            home.request_sha256,
        ),
    ];
    if agent
        .work
        .receipts
        .iter()
        .zip(expected)
        .any(|(receipt, (step, id, digest))| {
            receipt.step != step || receipt.operation_id != id || receipt.request_sha256 != digest
        })
        || agent.work.cleanup_debt.len() != 1
        || agent.work.cleanup_debt[0].step != AgentWorkStep::OpenConversation
        || agent.work.cleanup_debt[0].operation_id != open.operation_id
        || agent.work.cleanup_debt[0].request_sha256 != open.request_sha256
    {
        return Ok(None);
    }
    Ok(Some(record.payload.revision))
}
