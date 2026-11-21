use super::*;

use crate::sandbox::workspace_worker::{
    WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies,
};

fn replica(harness: &Harness) -> Arc<dyn SandboxAdapter> {
    let mut spec = CodexSandboxSpec::default();
    spec.deployment.image.uri = "registry.example/runtime:next".into();
    Arc::new(CodexAdapter::with_backend(harness.backend.clone(), spec))
}

fn replica_worker(harness: &Harness, adapter: Arc<dyn SandboxAdapter>) -> Arc<AgentWorker> {
    let registry = Arc::new(LiveAgentRegistry::default());
    AgentWorker::new(
        AgentWorkerDependencies {
            store: harness.store.clone(),
            adapter,
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(harness.store.clone()),
            operations: AgentOperationService::new(harness.store.clone(), registry),
        },
        AgentWorkerConfig::new("another-replica", DEPLOYMENT_ID),
    )
}

#[tokio::test]
async fn agent_startup_adopts_the_bound_replacement_generation_and_persisted_image() {
    let harness = Harness::with_generation(Arc::new(EmptyAgentSkillResolver), 4).await;
    let agent = harness.admit("replacement-runtime", Vec::new()).await;
    let adapter = replica(&harness);
    let worker = replica_worker(&harness, adapter.clone());
    for _ in 0..5 {
        assert!(worker.run_one(&agent.id, harness.now).await.unwrap());
    }
    assert_eq!(harness.stored(&agent.id).await.status, AgentStatus::Ready);
    let inventory = adapter.runtime_inventory().await.unwrap();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].generation, 4);
    assert_eq!(inventory[0].handle, harness.runtime);
}

#[tokio::test]
async fn absent_runtime_leaves_agent_startup_retryable_without_reprovisioning() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("missing-runtime", Vec::new()).await;
    let adapter = replica(&harness);
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        adapter.as_ref(),
        &harness
            .store
            .get_sandbox(&agent.sandbox_id)
            .await
            .unwrap()
            .unwrap(),
        DEPLOYMENT_ID,
    )
    .await
    .unwrap();
    adapter.teardown(&harness.runtime).await.unwrap();
    let worker = replica_worker(&harness, adapter.clone());
    assert!(worker.run_one(&agent.id, harness.now).await.unwrap());
    let retry = harness.stored(&agent.id).await;
    assert_eq!(retry.status, AgentStatus::Starting);
    assert_eq!(retry.work.step, AgentWorkStep::AddWorktree);
    assert!(adapter.runtime_inventory().await.unwrap().is_empty());
}

#[tokio::test]
async fn foreign_parent_is_rejected_before_runtime_adoption() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("foreign-parent", Vec::new()).await;
    let mut workspace = harness
        .store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let revision = workspace.revision;
    workspace.owner = SubjectId::new("foreign-owner");
    workspace.revision += 1;
    harness
        .store
        .save_workspace(revision, workspace)
        .await
        .unwrap();

    // Runtime adoption would fail retryably. Invalid lineage must win and
    // terminate startup before the worker attempts to use that runtime.
    harness.adapter.teardown(&harness.runtime).await.unwrap();
    harness.run(&agent.id, harness.now).await;
    let rejected = harness.stored(&agent.id).await;
    assert_eq!(rejected.status, AgentStatus::Failed);
    assert_eq!(rejected.failure.unwrap().class, "agent_lineage_invalid");
    assert!(rejected.work.receipts.is_empty());
}

#[tokio::test]
async fn workspace_preparation_uses_the_exact_bound_replacement_runtime() {
    let harness = Harness::with_generation(Arc::new(EmptyAgentSkillResolver), 4).await;
    let mut workspace = harness.workspace.clone();
    workspace.id = WorkspaceId::generate();
    workspace.name = "second".into();
    workspace.path = "/home/cma/second".into();
    workspace.profile.content.name = "second".into();
    workspace.profile.content.location = WorkspaceLocation::Directory {
        name: "second".into(),
    };
    workspace.resolved_commit = None;
    workspace.status = WorkspaceStatus::Preparing;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, rfc3339_at(harness.now));
    harness
        .store
        .create_workspace(workspace.clone())
        .await
        .unwrap();
    let adapter = replica(&harness);
    let worker = WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: harness.store.clone(),
            adapter: adapter.clone(),
        },
        WorkspaceWorkerConfig::new("another-replica", DEPLOYMENT_ID),
    );
    let ready = worker
        .run_until_ready(&workspace.id, harness.now)
        .await
        .unwrap()
        .workspace;
    assert_eq!(ready.status, WorkspaceStatus::Ready);
    let inventory = adapter.runtime_inventory().await.unwrap();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].generation, 4);
    assert_eq!(inventory[0].handle, harness.runtime);
}
