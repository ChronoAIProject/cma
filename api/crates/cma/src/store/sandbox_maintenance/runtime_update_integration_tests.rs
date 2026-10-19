use super::*;
use layer2_core::sandbox::store::{AgentWorkerStore, WorkspaceWorkerStore};

fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace {
        id: WorkspaceId(format!("wks_{}", sandbox.id)),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_runtime_update".into()),
            revision: 1,
            digest: "d".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "update".into(),
                },
                name: "update".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "update".into(),
        path: "/home/cma/update".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("a".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, IDLE_AT.into()),
        next_worktree_number: 2,
        created_at: IDLE_AT.into(),
        ready_at: Some(IDLE_AT.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_update_mongo_defers_work_and_fences_all_children_when_idle() {
    let (client, name, store) = database().await;
    for scenario in ["turn", "cleanup", "workspace", "workspace_lease", "idle"] {
        let sandbox = active_sandbox(&format!("sbx_update_{scenario}"));
        store.create_sandbox(sandbox.clone()).await.unwrap();
        let mut agent = ready_agent(&sandbox, &format!("agt_{scenario}"));
        let mut workspace = workspace(&sandbox);
        match scenario {
            "turn" => {
                agent.status = AgentStatus::Working;
                agent.conversation.active_turn = Some(2);
            }
            "cleanup" => {
                agent.status = AgentStatus::Stopped;
                agent
                    .work
                    .schedule(AgentWorkStep::RemoveHome, IDLE_AT.into());
            }
            "workspace" | "workspace_lease" => {
                workspace
                    .work
                    .schedule(WorkspaceWorkStep::Remove, IDLE_AT.into());
                if scenario == "workspace_lease" {
                    workspace
                        .work
                        .claim("removing".into(), IDLE_AT, LATER.into())
                        .unwrap();
                }
            }
            _ => {
                agent.status = AgentStatus::Stopped;
            }
        }
        insert_agent(&store, &agent, IDLE_AT).await;
        store.create_workspace(workspace.clone()).await.unwrap();
        assert_eq!(
            store
                .schedule_image_rotation(rotation(&sandbox.id))
                .await
                .unwrap(),
            if scenario == "idle" {
                SandboxMaintenanceOutcome::Scheduled
            } else {
                SandboxMaintenanceOutcome::NotEligible
            }
        );
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap(),
            Some(agent.clone())
        );
        assert_eq!(
            store.get_workspace(&workspace.id).await.unwrap(),
            Some(workspace.clone())
        );
        if scenario == "idle" {
            for (collection, id) in [
                (&store.agents, agent.id.as_str()),
                (&store.workspaces, workspace.id.as_str()),
            ] {
                let row = collection
                    .find_one(doc! {"_id": id})
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(row.get_i64("sandbox_maintenance_fence").unwrap(), 1);
            }
            // A stale new writer cannot acquire runtime authority after admission.
            let mut pending = agent.clone();
            pending
                .work
                .schedule(AgentWorkStep::RemoveHome, IDLE_AT.into());
            store
                .agents
                .replace_one(
                    doc! {"_id": agent.id.as_str()},
                    agent_document(&pending).unwrap(),
                )
                .await
                .unwrap();
            assert!(store
                .claim_agent_work(
                    &agent.id,
                    "late",
                    LATER.into(),
                    "2026-09-05T00:40:00Z".into()
                )
                .await
                .unwrap()
                .is_none());
            let mut pending_workspace = workspace.clone();
            pending_workspace
                .work
                .schedule(WorkspaceWorkStep::Remove, IDLE_AT.into());
            store
                .workspaces
                .replace_one(
                    doc! {"_id": workspace.id.as_str()},
                    crate::store::workspaces::workspace_document(&pending_workspace).unwrap(),
                )
                .await
                .unwrap();
            assert!(store
                .claim_workspace_work(
                    &workspace.id,
                    "late",
                    LATER.into(),
                    "2026-09-05T00:40:00Z".into()
                )
                .await
                .unwrap()
                .is_none());
        } else {
            assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_update_mongo_fence_conflicts_with_a_turn_after_idle_snapshot() {
    let (client, name, store) = database().await;
    let (sandbox, ready) = fixture(&store, "sbx_update_turn_race").await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    evidence::live_activities(&store, std::slice::from_ref(&ready), &mut session)
        .await
        .unwrap();
    let mut working = ready.clone();
    working
        .transition(AgentStatus::Working, LATER.into(), None)
        .unwrap();
    working.conversation.active_turn = Some(2);
    working.conversation.last_seq = 3;
    store
        .commit_agent_conversation(AgentConversationMutation {
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: Some(layer2_core::sandbox::store::SandboxMutationGuard::capture(
                &sandbox,
            )),
            expected_agent_revision: ready.revision,
            expected_last_seq: ready.conversation.last_seq,
            runtime_generation: 1,
            agent: working.clone(),
            appended_at: LATER.into(),
            agent_logs: vec![
                AgentLogKind::TurnStarted {
                    model: None,
                    turn_sequence: 2,
                    origin_key: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "Continue work.".into(),
                },
            ],
        })
        .await
        .unwrap();
    let result = transaction::apply(
        &store,
        &MaintenanceRequest::Rotation(rotation(&sandbox.id)),
        &mut session,
    )
    .await;
    assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store
            .schedule_image_rotation(rotation(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
    assert_eq!(store.get_agent(&ready.id).await.unwrap(), Some(working));
    client.database(&name).drop().await.unwrap();
}
