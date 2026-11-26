use super::*;
use layer2_core::sandbox::store::AgentSkillResultGuard;

pub(in crate::rest) async fn prepare_parents(store: &InMemorySandboxStore, agent: &Agent) {
    let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = sandbox.revision;
    sandbox.credential.key_generation = 1;
    sandbox.credential.delivered = true;
    sandbox.credential.current = Some(layer2_core::sandbox::SandboxCredentialAuthority {
        generation: 1,
        provider_key_id: "fixture-skill-credential-reference".into(),
        provider_state_version: 1,
        issued_at: NOW.into(),
        expires_at: "2026-09-06T00:00:00Z".into(),
        grant_fingerprint: "b".repeat(64),
    });
    sandbox.revision += 1;
    sandbox.validate().unwrap();
    store.save_sandbox(revision, sandbox).await.unwrap();

    let workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let mut worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let revision = worktree.revision;
    worktree.path = format!("{}/.worktrees/{}", workspace.path, worktree.name);
    worktree.branch = format!("cma/{}", worktree.name);
    worktree.revision += 1;
    store.save_worktree(revision, worktree).await.unwrap();
}

pub(in crate::rest) async fn capture(
    store: &InMemorySandboxStore,
    agent: &Agent,
) -> AgentSkillResultGuard {
    let sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    AgentSkillResultGuard::capture(agent, &sandbox, &workspace, &worktree).unwrap()
}
