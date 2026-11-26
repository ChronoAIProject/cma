//! Durable early hierarchy publication and exact restart ownership.
use super::*;
use layer2_core::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore};
use layer2_core::sandbox::{Agent, AgentStatus, AgentWorkStep, Workspace, WorkspaceOrigin};
use layer2_core::WorkspaceId;
use mongodb::bson::{doc, DateTime, Document};

#[path = "agent_lifecycle_bootstrap_fixture.rs"]
mod fixture;
use fixture::{work, Fixture};
#[path = "agent_lifecycle_bootstrap_failure_tests.rs"]
mod failure_tests;
#[path = "agent_lifecycle_bootstrap_title_tests.rs"]
mod title_tests;
#[path = "agent_lifecycle_bootstrap_upgrade_tests.rs"]
mod upgrade_tests;

fn start(
    service: Arc<InlineAgentLifecycle>,
    caller: Caller,
    input: ProvisionInput,
) -> tokio::task::JoinHandle<Result<String, AgentLifecycleProvisioningError>> {
    tokio::spawn(async move { service.provision(&caller, input).await })
}

async fn cancel(task: tokio::task::JoinHandle<Result<String, AgentLifecycleProvisioningError>>) {
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn bootstrap_facade_publishes_exact_hierarchy_before_runtime_and_replays_lost_agent_mark() {
    let f = Fixture::new().await;
    let task = start(f.service.clone(), f.owner.caller.clone(), work(&f.record));
    let (recorded, agent, workspace) = f.published().await;
    assert!(!task.is_finished());
    assert_eq!(agent.status, AgentStatus::Starting);
    assert_eq!(
        recorded.input.setup_message,
        Some(validation::setup_message(&recorded.input.request))
    );
    assert_eq!(
        agent
            .conversation
            .first_message
            .as_ref()
            .map(|message| &message.text),
        recorded.input.setup_message.as_ref()
    );
    assert_eq!(agent.work.step, AgentWorkStep::WaitForWorkspace);
    assert_eq!(agent.sandbox_id, f.sandbox.id);
    assert_eq!(workspace.status, WorkspaceStatus::Preparing);
    assert_eq!(workspace.resolved_commit, None);
    assert_eq!(
        workspace.origin,
        WorkspaceOrigin::SandboxProfile { index: 0 }
    );
    assert_eq!(workspace.profile.id, f.workspace_profile.id);
    assert_eq!(workspace.profile.revision, f.workspace_profile.revision);
    assert_eq!(recorded.agent_id.as_deref(), Some(agent.id.as_str()));
    let parent = f.store.get_sandbox(&f.sandbox.id).await.unwrap().unwrap();
    assert!(parent.revision > f.sandbox.revision);
    let mut expected_parent = f.sandbox.clone();
    expected_parent.revision = parent.revision;
    expected_parent.updated_at.clone_from(&parent.updated_at);
    expected_parent
        .last_activity_at
        .clone_from(&parent.last_activity_at);
    assert_eq!(
        parent, expected_parent,
        "admission may only record activity"
    );
    let worktree = f
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert!(worktree.base_commit.is_empty());
    assert!(!recorded.events.iter().any(|event| {
        ["workspace_runtime", "agent_startup"].contains(&event.phase.as_str())
            && event.status == "ready"
    }));
    cancel(task).await;

    // Simulate the crash boundary between atomic Agent admission and its
    // lifecycle mark, then expire the old worker lease. A reopened facade must
    // claim a fresh lease and recover the exact admitted origin from storage.
    f.db.collection::<Document>("agent_lifecycle_operations")
        .update_one(
            doc! {"_id": &recorded.id},
            doc! {"$set": {"agent_id": null, "lease_expires_at": DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let restarted = f.restart().await;
    let reclaimed = restarted
        .operations
        .claim(&recorded.id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(reclaimed.lease_token, recorded.lease_token);
    assert!(f
        .operations
        .mark_agent(&recorded.lease().unwrap(), agent.id.as_str())
        .await
        .is_err());
    let replay = start(restarted, f.owner.caller.clone(), work(&reclaimed));
    let (_, replayed, same_workspace) = f.published().await;
    assert!(!replay.is_finished());
    assert_eq!(replayed.id, agent.id);
    assert_eq!(
        replayed.conversation.first_message,
        agent.conversation.first_message
    );
    assert_eq!(same_workspace.id, workspace.id);
    assert_eq!(
        f.store
            .list_agents(&f.owner.caller.subject, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        f.store
            .list_workspaces(&f.sandbox.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    cancel(replay).await;
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn bootstrap_facade_active_parent_replay_retains_original_agent_and_exact_workspace_wait() {
    let f = Fixture::new().await;
    let task = start(f.service.clone(), f.owner.caller.clone(), work(&f.record));
    let (recorded, agent, workspace) = f.published().await;
    cancel(task).await;
    let mut parent = f.store.get_sandbox(&f.sandbox.id).await.unwrap().unwrap();
    let revision = parent.revision;
    parent.status = SandboxStatus::Active;
    parent.revision += 1;
    f.store.save_sandbox(revision, parent).await.unwrap();
    // An unrelated Ready Workspace cannot release this recorded Agent.
    let mut other = workspace.clone();
    other.id = WorkspaceId("wk_unrelated_ready".into());
    other.name = "unrelated-ready".into();
    other.path = "/home/cma/unrelated-ready".into();
    other.origin = WorkspaceOrigin::Manual;
    other.record_resolved_commit("a".repeat(40)).unwrap();
    other
        .transition(WorkspaceStatus::Ready, layer2_core::now_rfc3339(), None)
        .unwrap();
    other.revision = 1;
    f.store.create_workspace(other).await.unwrap();
    let replay = start(f.restart().await, f.owner.caller.clone(), work(&recorded));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!replay.is_finished());
    assert!(!f
        .record()
        .await
        .events
        .iter()
        .any(|event| { event.phase == "workspace_runtime" && event.status == "ready" }));
    cancel(replay).await;

    let mut ready_workspace = workspace.clone();
    ready_workspace
        .record_resolved_commit("b".repeat(40))
        .unwrap();
    ready_workspace
        .transition(WorkspaceStatus::Ready, layer2_core::now_rfc3339(), None)
        .unwrap();
    f.store
        .save_workspace(workspace.revision, ready_workspace)
        .await
        .unwrap();
    // The parent can reach real readiness before the Agent worker binds its
    // reserved Worktree. Lost-mark recovery must still reuse that original.
    f.db.collection::<Document>("agent_lifecycle_operations")
        .update_one(
            doc! {"_id": &recorded.id},
            doc! {"$set": {"agent_id": null}},
        )
        .await
        .unwrap();
    let missing_mark = f.record().await;
    let lost_mark = start(
        f.restart().await,
        f.owner.caller.clone(),
        work(&missing_mark),
    );
    let (_, recovered, recovered_workspace) = f.published().await;
    assert_eq!(recovered.id, agent.id);
    assert_eq!(recovered_workspace.id, workspace.id);
    assert_eq!(recovered.work.step, AgentWorkStep::WaitForWorkspace);
    assert!(!lost_mark.is_finished());
    cancel(lost_mark).await;
    // Recorded-id replay does not ask an Active parent to create another
    // profile Workspace. The original Agent remains the sole startup target.
    let mut ready_worktree = f
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let worktree_revision = ready_worktree.revision;
    ready_worktree.base_commit = "b".repeat(40);
    ready_worktree
        .transition(layer2_core::sandbox::WorktreeStatus::Ready)
        .unwrap();
    f.store
        .save_worktree(worktree_revision, ready_worktree)
        .await
        .unwrap();
    let mut ready_agent = agent.clone();
    ready_agent.work.step = AgentWorkStep::None;
    ready_agent.conversation.conversation_id = Some("original-conversation".into());
    ready_agent.conversation.runtime_generation = Some(1);
    ready_agent
        .transition(AgentStatus::Ready, layer2_core::now_rfc3339(), None)
        .unwrap();
    f.store
        .save_agent(agent.revision, ready_agent)
        .await
        .unwrap();
    let id = tokio::time::timeout(
        Duration::from_secs(5),
        f.restart()
            .await
            .provision(&f.owner.caller, work(&recorded)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(id, agent.id.as_str());
    assert_eq!(
        f.record().await.agent_id.as_deref(),
        Some(agent.id.as_str())
    );
    assert_eq!(
        f.store
            .list_agents(&f.owner.caller.subject, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn bootstrap_facade_concurrent_workspace_reservation_reuses_exact_profile_origin() {
    let f = Fixture::new().await;
    let profile = SandboxProfileRef {
        id: f.sandbox.profile.id.clone(),
        revision: f.sandbox.profile.revision,
    };
    let reserve = || {
        f.service.reserve_bootstrap_workspace(
            &f.owner.caller,
            &f.sandbox.id,
            &profile,
            &f.workspace_profile,
            &f.record.id,
        )
    };
    let (first, second) = tokio::join!(reserve(), reserve());
    assert_eq!(first.unwrap(), second.unwrap());
    let children = f
        .store
        .list_workspaces(&f.sandbox.id, &Default::default())
        .await
        .unwrap();
    assert_eq!(children.items.len(), 1);
    assert_eq!(
        children.items[0].origin,
        WorkspaceOrigin::SandboxProfile { index: 0 }
    );
    assert_eq!(children.items[0].status, WorkspaceStatus::Preparing);
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn bootstrap_facade_rejects_foreign_and_mismatched_recorded_hierarchy() {
    let f = Fixture::new().await;
    let profile = SandboxProfileRef {
        id: f.sandbox.profile.id.clone(),
        revision: f.sandbox.profile.revision,
    };
    let mut foreign = f.owner.caller.clone();
    foreign.subject = SubjectId::new("foreign");
    assert!(matches!(
        f.service
            .reserve_bootstrap_workspace(
                &foreign,
                &f.sandbox.id,
                &profile,
                &f.workspace_profile,
                &f.record.id,
            )
            .await,
        Err(AgentLifecycleProvisioningError::Conflict(_))
    ));
    assert!(matches!(
        f.service
            .reserve_bootstrap_workspace(
                &f.owner.caller,
                &f.sandbox.id,
                &profile,
                &f.workspace_profile,
                "different-operation",
            )
            .await,
        Err(AgentLifecycleProvisioningError::Conflict(_))
    ));
    for change in ["profile", "workspace_profile"] {
        let mut parent = f.store.get_sandbox(&f.sandbox.id).await.unwrap().unwrap();
        let revision = parent.revision;
        match change {
            "profile" => parent.profile.revision += 1,
            "workspace_profile" => parent.profile.content.workspace_profiles[0].revision += 1,
            _ => unreachable!(),
        }
        parent.revision += 1;
        f.store
            .save_sandbox(revision, parent.clone())
            .await
            .unwrap();
        let error = f
            .service
            .provision(&f.owner.caller, work(&f.record))
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                AgentLifecycleProvisioningError::Conflict(_)
                    | AgentLifecycleProvisioningError::Forbidden
            ),
            "{change}: {error:?}"
        );
        assert!(f
            .store
            .list_agents(&f.owner.caller.subject, &Default::default())
            .await
            .unwrap()
            .items
            .is_empty());
        assert!(f
            .store
            .list_workspaces(&f.sandbox.id, &Default::default())
            .await
            .unwrap()
            .items
            .is_empty());
        let mut restored = f.sandbox.clone();
        restored.revision = parent.revision + 1;
        f.store
            .save_sandbox(parent.revision, restored)
            .await
            .unwrap();
    }
    let task = start(f.service.clone(), f.owner.caller.clone(), work(&f.record));
    let (recorded, agent, workspace) = f.published().await;
    cancel(task).await;
    for change in ["sandbox", "workspace", "origin"] {
        let mut mismatched = agent.clone();
        match change {
            "sandbox" => mismatched.sandbox_id = SandboxId("sbx_other".into()),
            "workspace" => mismatched.workspace_id = WorkspaceId("wk_other".into()),
            "origin" => {
                mismatched.origin = AgentOrigin::Lifecycle {
                    operation_id: "other".into(),
                }
            }
            _ => unreachable!(),
        }
        assert!(matches!(
            crate::agent_lifecycle::bootstrap::validate_recorded_agent(
                &mismatched,
                &f.sandbox.id,
                &workspace.id,
                &recorded.id,
            ),
            Err(AgentLifecycleProvisioningError::Conflict(_))
        ));
    }
    let mut mismatched = workspace.clone();
    mismatched.profile.revision += 1;
    mismatched.revision += 1;
    f.store
        .save_workspace(workspace.revision, mismatched)
        .await
        .unwrap();
    assert!(matches!(
        f.service.provision(&f.owner.caller, work(&recorded)).await,
        Err(AgentLifecycleProvisioningError::Conflict(_))
    ));
    f.db.drop().await.unwrap();
}
