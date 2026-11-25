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

pub(in crate::sandbox) async fn unbound_worker() -> (Arc<AgentWorker>, Agent) {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("inspection-correlation", vec![]).await;
    let mut sandbox = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let expected = sandbox.revision;
    sandbox.runtime = None;
    sandbox.revision += 1;
    harness.store.save_sandbox(expected, sandbox).await.unwrap();
    (harness.worker, agent)
}

/// Run the real inspection path against an intentionally absent runtime.
pub(crate) async fn missing_runtime(step: AgentWorkStep) -> (Arc<AgentWorker>, Agent) {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let mut agent = harness.admit("inspection-diagnostics", vec![]).await;
    agent.work.step = step;
    harness.adapter.teardown(&harness.runtime).await.unwrap();
    (harness.worker, agent)
}
