use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_agent_fence_conflicts_with_a_turn_after_the_evidence_snapshot() {
    let (client, name, store) = database().await;
    let (sandbox, ready) = fixture(&store, "sbx_turn_race").await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let evidence =
        evidence::live_activities(&store, std::slice::from_ref(&ready), &mut session, LATER)
            .await
            .unwrap();
    assert!(layer2_core::sandbox::is_inactivity_pause_candidate(
        &sandbox, &evidence, LATER, 900
    ));
    let mut working = ready.clone();
    working
        .transition(AgentStatus::Working, LATER.into(), None)
        .unwrap();
    working.conversation.active_turn = Some(2);
    working.conversation.last_seq = 3;
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
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
                    content: "Continue the investigation.".into(),
                },
            ],
        })
        .await
        .unwrap();
    let result = transaction::apply(
        &store,
        &MaintenanceRequest::Inactivity(pause(&sandbox.id)),
        &mut session,
    )
    .await;
    assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
    assert_eq!(store.get_agent(&ready.id).await.unwrap(), Some(working));
    assert_eq!(store.agent_log(&ready.id, 0, 10).await.unwrap().len(), 3);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_fence_conflicts_with_a_new_log_at_the_same_agent_revision() {
    let (client, name, store) = database().await;
    let (sandbox, ready) = fixture(&store, "sbx_log_race").await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    evidence::live_activities(&store, std::slice::from_ref(&ready), &mut session, LATER)
        .await
        .unwrap();
    store
        .append_agent_log(
            &ready.id,
            LATER.into(),
            AgentLogKind::OwnerMessage {
                content: "Check the new request.".into(),
            },
        )
        .await
        .unwrap();
    let current = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(current.revision, ready.revision);
    assert_eq!(
        current.conversation.last_seq,
        ready.conversation.last_seq + 1
    );
    let result = transaction::apply(
        &store,
        &MaintenanceRequest::Inactivity(pause(&sandbox.id)),
        &mut session,
    )
    .await;
    assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_sandbox_write_conflicts_with_a_newly_admitted_agent() {
    let (client, name, store) = database().await;
    let (sandbox, ready) = fixture(&store, "sbx_admission_race").await;
    let workspace = Workspace {
        id: ready.workspace_id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_maintenance".into()),
            revision: 1,
            digest: "d".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "maintenance".into(),
                },
                name: "maintenance".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "maintenance".into(),
        path: "/home/cma/maintenance".into(),
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
    };
    store.create_workspace(workspace.clone()).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    evidence::live_activities(&store, std::slice::from_ref(&ready), &mut session, LATER)
        .await
        .unwrap();
    store
        .admit_agent(AgentAdmission {
            binding: None,
            created_by: None,
            owner: sandbox.owner.clone(),
            workspace_id: workspace.id,
            agent_id: AgentId("agt_newly_admitted".into()),
            worktree_id: WorktreeId("wkt_newly_admitted".into()),
            profile: ready.profile.clone(),
            capabilities: ready.capabilities.clone(),
            title: None,
            first_message: None,
            origin: AgentOrigin::Manual,
            creation_key: "new-during-maintenance".into(),
            event_at: LATER.into(),
        })
        .await
        .unwrap();
    let result = transaction::apply(
        &store,
        &MaintenanceRequest::Inactivity(pause(&sandbox.id)),
        &mut session,
    )
    .await;
    assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        store
            .get_sandbox(&sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Active
    );
    let row = store
        .agents
        .find_one(doc! { "_id": ready.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(!row.contains_key("sandbox_maintenance_fence"));
    assert_eq!(
        store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        2
    );
    client.database(&name).drop().await.unwrap();
}
