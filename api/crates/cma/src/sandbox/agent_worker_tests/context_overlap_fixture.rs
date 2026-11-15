//! Real parent records for the context orchestration contract.
use super::*;

pub(crate) async fn parents() -> (Agent, Sandbox, Workspace, layer2_core::sandbox::Worktree) {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("context-overlap", vec![]).await;
    let sandbox = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let workspace = harness
        .store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = harness
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    (agent, sandbox, workspace, worktree)
}
