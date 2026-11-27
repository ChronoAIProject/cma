use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fresh_database_initializes_every_schema_v1_collection_and_index() {
    let (client, name) = database("fresh").await;
    let sandbox_store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let profile_store = MongoProfileStore::open(&client, &name).await.unwrap();
    let _credential_store =
        MongoSandboxCredentialStore::open(&client, &name, "test", &encryption_key())
            .await
            .unwrap();
    let names = client
        .database(&name)
        .list_collection_names()
        .await
        .unwrap();
    for required in [
        SANDBOXES,
        WORKSPACES,
        WORKTREES,
        AGENTS,
        AGENT_LOG,
        SANDBOX_EVENTS,
        SANDBOX_ADMISSIONS,
        SANDBOX_CREDENTIALS,
        SANDBOX_CREDENTIAL_REVOCATIONS,
        CHECKPOINTS,
        AGENT_PROFILES,
        AGENT_PROFILE_REVISIONS,
        WORKSPACE_PROFILES,
        WORKSPACE_PROFILE_REVISIONS,
        SANDBOX_PROFILES,
        SANDBOX_PROFILE_REVISIONS,
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "missing {required}"
        );
    }
    assert!(sandbox_store
        .sandboxes
        .list_index_names()
        .await
        .unwrap()
        .contains(&"sandboxes_owner_name_live".to_string()));
    assert!(sandbox_store
        .agents
        .list_index_names()
        .await
        .unwrap()
        .contains(&"agents_owner_creation_key".to_string()));
    assert!(profile_store
        .agent_revisions
        .list_index_names()
        .await
        .unwrap()
        .contains(&"agent_profile_revisions_profile_revision".to_string()));
    assert!(client
        .database(&name)
        .collection::<Document>(SANDBOX_CREDENTIALS)
        .list_index_names()
        .await
        .unwrap()
        .contains(&"sandbox_credentials_generation".to_string()));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_admission_serializes_the_limit_and_default_name_allocation() {
    let (client, name) = database("admission").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let results = futures_util::future::join_all((0..6).map(|index| {
        let store = store.clone();
        async move {
            store
                .admit_sandbox(admission(&format!("sbx_admission_{index}"), 3))
                .await
        }
    }))
    .await;
    let mut admitted = Vec::new();
    for result in results {
        match result {
            Ok(SandboxAdmissionResult::Created(sandbox)) => admitted.push(sandbox),
            Err(SandboxStoreError::LimitReached {
                limit: 3, live: 3, ..
            }) => {}
            other => panic!("unexpected concurrent admission result: {other:?}"),
        }
    }
    assert_eq!(admitted.len(), 3);
    let mut names = admitted
        .iter()
        .map(|sandbox| sandbox.name.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, ["research", "research-2", "research-3"]);
    for sandbox in admitted {
        let events = store.sandbox_events(&sandbox.id, 0, 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[0].kind, SandboxEventKind::Created);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_workspace_admission_serializes_limits_and_live_names() {
    let (client, name) = database("workspace_admission").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_workspace_admission");
    parent.limits.max_workspaces = 2;
    store.create_sandbox(parent.clone()).await.unwrap();

    let results = futures_util::future::join_all((0..6).map(|index| {
        let store = store.clone();
        let admission = workspace_admission(workspace(
            &parent,
            &format!("wks_concurrent_{index}"),
            &format!("project-{index}"),
        ));
        async move { store.admit_workspace(admission).await }
    }))
    .await;
    let mut admitted = Vec::new();
    for result in results {
        match result {
            Ok(WorkspaceAdmissionResult::Created(workspace)) => admitted.push(workspace),
            Err(SandboxStoreError::WorkspaceLimitReached { limit: 2, live: 2 }) => {}
            other => panic!("unexpected workspace admission result: {other:?}"),
        }
    }
    assert_eq!(admitted.len(), 2);
    assert_eq!(
        store
            .list_workspaces(&parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    assert_eq!(
        store.sandbox_events(&parent.id, 0, 10).await.unwrap().len(),
        2
    );

    let mut other = sandbox("sbx_workspace_name");
    other.name = "workspace-name-race".into();
    store.create_sandbox(other.clone()).await.unwrap();
    let left = store.admit_workspace(workspace_admission(workspace(
        &other,
        "wks_name_left",
        "same-name",
    )));
    let right = store.admit_workspace(workspace_admission(workspace(
        &other,
        "wks_name_right",
        "same-name",
    )));
    let (left, right) = tokio::join!(left, right);
    let results = [left, right];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(WorkspaceAdmissionResult::Created(_))))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(SandboxStoreError::WorkspaceNameConflict)))
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_agent_admission_at_limit_minus_one_admits_exactly_one() {
    let (client, name) = database("agent_admission_limit").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_agent_admission_limit");
    parent.limits.max_agents_per_workspace = 2;
    parent.profile.content.max_agents_per_workspace = 2;
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = ready_workspace(&parent, "wks_agent_admission_limit", "agent-limit");
    store.create_workspace(workspace.clone()).await.unwrap();
    store
        .admit_agent(agent_admission(&workspace, "first", "limit_first"))
        .await
        .unwrap();

    let left = store.admit_agent(agent_admission(&workspace, "left", "limit_left"));
    let right = store.admit_agent(agent_admission(&workspace, "right", "limit_right"));
    let (left, right) = tokio::join!(left, right);
    let results = [left, right];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(AgentAdmissionResult::Created(_))))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                Err(SandboxStoreError::AgentLimitReached { limit: 2, live: 2 })
            ))
            .count(),
        1
    );
    let stored_workspace = store.get_workspace(&workspace.id).await.unwrap().unwrap();
    assert_eq!(stored_workspace.next_worktree_number, 3);
    assert_eq!(stored_workspace.revision, 3);
    assert_eq!(store.agents.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(store.worktrees.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        store.sandbox_events(&parent.id, 0, 10).await.unwrap().len(),
        4
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_admission_allocations_are_monotonic_and_never_reused() {
    let (client, name) = database("agent_admission_numbers").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_agent_admission_numbers");
    parent.limits.max_agents_per_workspace = 1;
    parent.profile.content.max_agents_per_workspace = 1;
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = ready_workspace(&parent, "wks_agent_admission_numbers", "agent-numbers");
    store.create_workspace(workspace.clone()).await.unwrap();

    let mut first = created_agent(
        store
            .admit_agent(agent_admission(&workspace, "number-1", "number_1"))
            .await
            .unwrap(),
    );
    first.agent.status = first.agent.status.transition(AgentStatus::Stopped).unwrap();
    first.agent.revision += 1;
    store.save_agent(1, first.agent.clone()).await.unwrap();
    let mut second = created_agent(
        store
            .admit_agent(agent_admission(&workspace, "number-2", "number_2"))
            .await
            .unwrap(),
    );
    second.agent.status = second
        .agent
        .status
        .transition(AgentStatus::Stopped)
        .unwrap();
    second.agent.revision += 1;
    store.save_agent(1, second.agent.clone()).await.unwrap();
    let third = created_agent(
        store
            .admit_agent(agent_admission(&workspace, "number-3", "number_3"))
            .await
            .unwrap(),
    );

    assert_eq!(first.worktree.name, "wt-1");
    assert_eq!(second.worktree.name, "wt-2");
    assert_eq!(third.worktree.name, "wt-3");
    assert_eq!(second.worktree.branch, "cma/wt-2");
    assert_eq!(
        third.worktree.path,
        "/home/cma/agent-numbers/.worktrees/wt-3"
    );
    assert_eq!(
        store
            .get_workspace(&workspace.id)
            .await
            .unwrap()
            .unwrap()
            .next_worktree_number,
        4
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_admission_replays_exact_keys_and_rejects_changed_payloads() {
    let (client, name) = database("agent_admission_replay").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_agent_admission_replay");
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = ready_workspace(&parent, "wks_agent_admission_replay", "agent-replay");
    store.create_workspace(workspace.clone()).await.unwrap();

    let request = agent_admission(&workspace, "stable-key", "replay_first");
    let first = created_agent(store.admit_agent(request).await.unwrap());
    let replay = agent_admission(&workspace, "stable-key", "replay_retry");
    assert_eq!(
        store.admit_agent(replay).await.unwrap(),
        AgentAdmissionResult::Existing(first.clone())
    );
    let mut changed = agent_admission(&workspace, "stable-key", "replay_changed");
    changed.title = Some("different immutable payload".into());
    assert_eq!(
        store.admit_agent(changed).await,
        Err(SandboxStoreError::AgentCreationConflict)
    );
    assert_eq!(store.agents.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(store.worktrees.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        store
            .get_workspace(&workspace.id)
            .await
            .unwrap()
            .unwrap()
            .next_worktree_number,
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn late_agent_admission_failure_rolls_back_every_artifact() {
    let (client, name) = database("agent_admission_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_agent_admission_rollback");
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = ready_workspace(&parent, "wks_agent_admission_rollback", "agent-rollback");
    store.create_workspace(workspace.clone()).await.unwrap();
    let conflicting = SandboxEvent {
        sandbox_id: parent.id.clone(),
        seq: 2,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandbox_events
        .insert_one(event_document(&conflicting).unwrap())
        .await
        .unwrap();
    let request = agent_admission(&workspace, "will-roll-back", "rollback");

    assert_eq!(
        store.admit_agent(request.clone()).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace)
    );
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    let parent_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent_row.get_i64("last_seq").unwrap(), 0);
    assert_eq!(store.get_agent(&request.agent_id).await.unwrap(), None);
    assert_eq!(
        store.get_worktree(&request.worktree_id).await.unwrap(),
        None
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        1
    );
    assert!(store
        .sandbox_events
        .find_one(doc! { "sandbox_id": parent.id.as_str(), "seq": 1_i64 })
        .await
        .unwrap()
        .is_none());
    client.database(&name).drop().await.unwrap();
}
