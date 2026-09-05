use super::*;

#[tokio::test]
async fn agent_control_requests_validate_exact_identity_lineage_status_and_revision() {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("control-cas")).await.unwrap());
    let agent = creation.agent;
    let original = agent.clone();

    let mut missing = stop_request(&agent, AgentStopReason::Owner);
    missing.target.agent_id = AgentId("agt_missing".into());
    assert_eq!(
        store.schedule_agent_stop(missing).await.unwrap_err(),
        AgentControlStoreError::NotFound
    );
    let mut foreign = stop_request(&agent, AgentStopReason::Owner);
    foreign.target.owner = SubjectId::new("stranger");
    assert_eq!(
        store.schedule_agent_stop(foreign).await.unwrap_err(),
        AgentControlStoreError::NotFound
    );
    for target in [
        {
            let mut target = AgentControlTarget::from(&agent);
            target.worktree_id = WorktreeId("wkt_other".into());
            target
        },
        {
            let mut target = AgentControlTarget::from(&agent);
            target.workspace_id = WorkspaceId("wks_other".into());
            target
        },
        {
            let mut target = AgentControlTarget::from(&agent);
            target.sandbox_id = SandboxId("sbx_other".into());
            target
        },
        {
            let mut target = AgentControlTarget::from(&agent);
            target.expected_status = AgentStatus::Ready;
            target
        },
        {
            let mut target = AgentControlTarget::from(&agent);
            target.expected_revision += 1;
            target
        },
    ] {
        assert_eq!(
            store
                .schedule_agent_stop(AgentStopRequest {
                    target,
                    reason: AgentStopReason::Owner,
                    requested_at: MINUTE.into(),
                })
                .await
                .unwrap_err(),
            AgentControlStoreError::Conflict
        );
    }
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(original));
}

#[tokio::test]
async fn agent_control_lineage_corruption_rolls_back_without_mutating_the_agent() {
    for level in ["worktree", "workspace", "sandbox", "agent_key"] {
        let store = ready_store(1).await;
        let creation = created(store.admit_agent(admission(level)).await.unwrap());
        let agent = creation.agent;
        let original = agent.clone();
        {
            let mut state = store.lock();
            match level {
                "worktree" => {
                    state.worktrees.get_mut(&agent.worktree_id).unwrap().owner =
                        SubjectId::new("stranger");
                }
                "workspace" => {
                    state
                        .workspaces
                        .get_mut(&agent.workspace_id)
                        .unwrap()
                        .sandbox_id = SandboxId("sbx_other".into());
                }
                "sandbox" => {
                    state.sandboxes.get_mut(&agent.sandbox_id).unwrap().owner =
                        SubjectId::new("stranger");
                }
                "agent_key" => {
                    state.agents.get_mut(&agent.id).unwrap().id = AgentId("agt_other".into());
                }
                _ => unreachable!(),
            }
        }
        assert!(matches!(
            store
                .schedule_agent_stop(stop_request(&agent, AgentStopReason::Owner))
                .await,
            Err(AgentControlStoreError::Corrupt(_))
        ));
        let stored = store.lock().agents.get(&agent.id).cloned().unwrap();
        if level == "agent_key" {
            assert_eq!(stored.id, AgentId("agt_other".into()));
        } else {
            assert_eq!(stored, original);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_control_concurrent_identical_stop_requests_mutate_once() {
    let store = Arc::new(ready_store(1).await);
    let creation = created(store.admit_agent(admission("stop-race")).await.unwrap());
    let request = stop_request(&creation.agent, AgentStopReason::Owner);
    let mut tasks = Vec::new();
    for _ in 0..32 {
        let store = Arc::clone(&store);
        let request = request.clone();
        tasks.push(tokio::spawn(async move {
            store.schedule_agent_stop(request).await.unwrap()
        }));
    }
    let mut scheduled = 0;
    for task in tasks {
        if matches!(task.await.unwrap(), AgentControlResult::Scheduled(_)) {
            scheduled += 1;
        }
    }
    assert_eq!(scheduled, 1);
    let stored = store.get_agent(&creation.agent.id).await.unwrap().unwrap();
    assert_eq!(stored.revision, creation.agent.revision + 1);
    assert_eq!(stored.work.step, AgentWorkStep::FenceInteraction);
}
