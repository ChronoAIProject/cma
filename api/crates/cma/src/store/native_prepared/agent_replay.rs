//! Exact consumed preparation eligibility; effects still replay their own fences.
use layer2_core::sandbox::{
    Agent, AgentOrigin, AgentStatus, AgentWorkStep, Sandbox, SandboxStatus, SandboxStoreError,
    Workspace, WorkspaceOrigin, WorkspaceStatus, Worktree, WorktreeStatus,
};
use layer3_adapter::{AgentHomeRequest, WorktreeRequest};
use mongodb::ClientSession;

use crate::store::sandbox_mongo::MongoSandboxStore;

pub(in crate::store) async fn replay_proof_revision(
    store: &MongoSandboxStore,
    agent: &Agent,
    sandbox: &Sandbox,
    workspace: &Workspace,
    tree: &Worktree,
    session: &mut ClientSession,
) -> Result<Option<u64>, SandboxStoreError> {
    if agent.status != AgentStatus::Starting
        || agent.work.step != AgentWorkStep::AddWorktree
        || agent.ready_at.is_some()
        || agent.deleted_at.is_some()
        || agent.requested_stop_reason.is_some()
        || agent.conversation.conversation_id.is_some()
        || agent.conversation.runtime_generation.is_some()
        || agent.conversation.active_turn.is_some()
        || agent.conversation.operation.is_some()
        || !agent.work.receipts.is_empty()
        || agent.work.resolved_skills.as_deref() != Some(&[])
        || !agent.profile.content.skills.is_empty()
        || !crate::agent_lifecycle::guidance::is_prepared_prompt(
            &agent.profile.content.system_prompt,
        )
        || sandbox.status != SandboxStatus::Active
        || !sandbox.credential.delivered
        || sandbox.lifecycle.checkpoint_capture.is_some()
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
    if !agent.work.cleanup_debt.is_empty() {
        let request = WorktreeRequest::new(
            "worktree:workspace:wt-1:add:v1",
            "workspace",
            "wt-1",
            tree.base_commit.clone(),
        );
        let home = AgentHomeRequest::new(
            format!("agent:{}:home:create:v1", agent.id),
            crate::sandbox::runtime_options(agent, sandbox, tree),
            agent.profile.content.system_prompt.clone(),
            Vec::new(),
        );
        if agent.work.cleanup_debt.len() != 2
            || agent
                .work
                .cleanup_debt
                .iter()
                .zip([
                    (
                        AgentWorkStep::AddWorktree,
                        request.operation_id,
                        request.request_sha256,
                    ),
                    (
                        AgentWorkStep::MaterializeHome,
                        home.operation_id,
                        home.request_sha256,
                    ),
                ])
                .any(|(debt, (step, operation, digest))| {
                    debt.step != step
                        || debt.operation_id != operation
                        || debt.request_sha256 != digest
                })
        {
            return Ok(None);
        }
    }
    Ok(Some(record.payload.revision))
}
