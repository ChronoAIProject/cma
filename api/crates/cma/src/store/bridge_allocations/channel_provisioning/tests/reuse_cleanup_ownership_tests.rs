use super::*;
use crate::sandbox::{WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies};
use layer3_adapter::{wire::RequestFrame, RuntimeHandle};
use std::time::Duration;

fn fresh_workspace(previous: &Workspace) -> Workspace {
    let now = layer2_core::now_rfc3339();
    Workspace {
        id: layer2_core::WorkspaceId::generate(),
        origin: WorkspaceOrigin::Manual,
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, now.clone()),
        next_worktree_number: 1,
        created_at: now,
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
        ..previous.clone()
    }
}

fn manual_agent(f: &CleanupFixture, workspace: &Workspace, key: &str) -> AgentAdmission {
    AgentAdmission {
        binding: None,
        created_by: Some(caller().actor),
        owner: workspace.owner.clone(),
        workspace_id: workspace.id.clone(),
        agent_id: layer2_core::AgentId::generate(),
        worktree_id: layer2_core::WorktreeId::generate(),
        profile: f.f.f.sibling.profile.clone(),
        capabilities: f.f.f.sibling.capabilities.clone(),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
        creation_key: key.into(),
        event_at: layer2_core::now_rfc3339(),
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_execution_excludes_new_ordinary_child_admission() {
    let mut f = CleanupFixture::new(false).await;
    f.reset().await;
    f.request_cleanup().await.unwrap();
    f.close_admission().await.unwrap();
    let resources = &f.f.f.original.resources;
    let target = resources
        .get_workspace(&f.original.placement.target().workspace_id)
        .await
        .unwrap()
        .unwrap();
    let before = f.prepare_removal().await.unwrap();
    let parent = f.f.sandbox().await;
    let agents = f
        .db()
        .collection::<Document>("agents")
        .count_documents(doc! {})
        .await
        .unwrap();
    for workspace in [&target, &f.f.f.workspace] {
        let admission = manual_agent(&f, workspace, "during-cleanup");
        assert!(resources.admit_agent(admission.clone()).await.is_err());
        assert!(resources
            .admit_agent_reusing_profile(admission)
            .await
            .is_err());
    }
    let mut new_workspace = fresh_workspace(&target);
    new_workspace.name = "during-cleanup".into();
    new_workspace.path = "/home/cma/during-cleanup".into();
    assert!(resources
        .admit_workspace(WorkspaceAdmission {
            binding: None,
            workspace: new_workspace,
            kind: WorkspaceAdmissionKind::Manual,
            event_at: layer2_core::now_rfc3339(),
        })
        .await
        .is_err());
    assert_eq!(
        f.db()
            .collection::<Document>("agents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        agents
    );
    assert_eq!(f.f.sandbox().await, parent);
    assert!(f.prepare_removal().await.unwrap() == before);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_restore_refuses_a_recreated_workspace_with_the_same_paths() {
    let mut f = CleanupFixture::new(false).await;
    f.reset().await;
    f.request_cleanup().await.unwrap();
    f.close_admission().await.unwrap();
    let tombstone = f.prepare_removal().await.unwrap();
    for _ in 0..4 {
        f.remove_step().await.unwrap();
    }
    let resources = &f.f.f.original.resources;
    let workspace = resources
        .get_workspace(&tombstone.seal.target.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let workspace_worker = WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: resources.clone(),
            adapter: f.f.f.original.adapter.clone(),
        },
        WorkspaceWorkerConfig::new("cleanup-path-reincarnation", "fixture-deployment"),
    );
    resources
        .begin_workspace_removal(&workspace.id, &workspace.owner, layer2_core::now_rfc3339())
        .await
        .unwrap();
    assert!(workspace_worker
        .run_one(&workspace.id, SystemTime::now())
        .await
        .unwrap());
    assert_eq!(
        resources
            .get_workspace(&workspace.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkspaceStatus::Removed
    );
    let next = fresh_workspace(&workspace);
    resources
        .admit_workspace(WorkspaceAdmission {
            binding: None,
            workspace: next.clone(),
            kind: WorkspaceAdmissionKind::Manual,
            event_at: layer2_core::now_rfc3339(),
        })
        .await
        .unwrap();
    let next = workspace_worker
        .run_until_ready(&next.id, SystemTime::now())
        .await
        .unwrap();
    assert_eq!(next.status, WorkspaceStatus::Ready);
    assert_ne!(next.id, workspace.id);
    assert_eq!(next.path, workspace.path);
    f.f.pause().await;
    let before = f.f.f.original.backend.requests().await.len();
    // Actual paused creation admission schedules revival. The fresh reservation
    // has no writer debt and cannot prevent checkpoint capture beforehand.
    let admission = manual_agent(&f, &next, "new-path-owner");
    let created = resources
        .admit_agent(admission)
        .await
        .unwrap()
        .creation()
        .clone();
    assert_eq!(created.worktree.path, tombstone.seal.worktree_path);
    assert_ne!(created.worktree.id, tombstone.seal.target.worktree_id);
    // The new incarnation has reserved the same path. Its worker must not be
    // needed to prove that a tombstone no longer owns this path; the adapter
    // also retains independent retirement fences for previously removed names.
    assert_eq!(created.agent.status, AgentStatus::Starting);
    assert!(resources
        .retired_agent_runtime_paths(&f.f.sandbox().await)
        .await
        .is_err());
    // Corrupt only the disposable fixture's indexed parent columns. Embedded
    // ownership still names this parent and must remain visible to the scan.
    let mut aliases = Vec::new();
    for (collection, id) in [
        ("workspaces", next.id.as_str()),
        ("worktrees", created.worktree.id.as_str()),
    ] {
        let row = f.row(collection, id).await;
        f.db()
            .collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$set":{"sandbox_id":"sbx_hidden_alias"}},
            )
            .await
            .unwrap();
        aliases.push((collection, row));
    }
    assert!(resources
        .retired_agent_runtime_paths(&f.f.sandbox().await)
        .await
        .is_err());
    for (collection, row) in aliases {
        f.db()
            .collection::<Document>(collection)
            .replace_one(doc! {"_id":row.get_str("_id").unwrap()}, row)
            .await
            .unwrap();
    }
    for tick in 0..12 {
        f.f.worker
            .run_one(
                &f.f.f.parent.id,
                SystemTime::now() + Duration::from_secs(tick * 31),
            )
            .await
            .unwrap();
    }
    let parent = f.f.sandbox().await;
    assert_eq!(parent.status, SandboxStatus::Reviving);
    let requests = f.f.f.original.backend.requests().await;
    assert!(!requests[before..].iter().any(|(_, request)| matches!(
        request,
        RequestFrame::RemoveWorktree(_) | RequestFrame::RemoveAgentHome(_)
    )));
    let runtime =
        f.f.f
            .original
            .backend
            .snapshot(&RuntimeHandle(parent.runtime.unwrap().runtime_id))
            .await
            .unwrap();
    assert_eq!(
        runtime.workspaces.get(&next.name).unwrap().workspace_id,
        next.id.as_str()
    );
    assert_eq!(
        resources
            .get_agent(&created.agent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Starting
    );
    f.close().await;
}
