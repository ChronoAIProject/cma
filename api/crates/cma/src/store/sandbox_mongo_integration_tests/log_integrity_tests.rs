use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn injected_event_failure_rolls_back_worktree_agent_and_sequence() {
    let (client, name) = database("atomic").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = sandbox("sbx_atomic");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let conflicting = SandboxEvent {
        sandbox_id: sandbox.id.clone(),
        seq: 1,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandbox_events
        .insert_one(event_document(&conflicting).unwrap())
        .await
        .unwrap();
    let (worktree, agent) = worktree_and_agent(&sandbox.id);
    let error = store
        .commit(SandboxTransaction::CreateWorktreeAndAgent {
            worktree: Box::new(worktree.clone()),
            agent: Box::new(agent.clone()),
            event_at: NOW.into(),
            event: SandboxEventKind::AgentCreated {
                workspace_id: worktree.workspace_id.clone(),
                worktree_id: worktree.id.clone(),
                agent_id: agent.id.clone(),
            },
        })
        .await
        .unwrap_err();
    assert_eq!(error, SandboxStoreError::SequenceConflict);
    assert_eq!(store.get_worktree(&worktree.id).await.unwrap(), None);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), None);
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! { "_id": sandbox.id.0 })
            .await
            .unwrap()
            .unwrap()
            .get_i64("last_seq")
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn missing_event_in_the_durable_range_fails_closed() {
    let (client, name) = database("gap").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = sandbox("sbx_gap");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let first = store
        .append_sandbox_event(&sandbox.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    let second = store
        .append_sandbox_event(&sandbox.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    assert_eq!((first.seq, second.seq), (1, 2));
    store
        .sandbox_events
        .delete_one(doc! { "sandbox_id": sandbox.id.as_str(), "seq": 1_i64 })
        .await
        .unwrap();
    assert_eq!(
        store.sandbox_events(&sandbox.id, 0, 10).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn missing_agent_log_entry_in_the_durable_range_fails_closed() {
    let (client, name) = database("agent_log_gap").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = sandbox("sbx_agent_log_gap");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let (_, agent) = worktree_and_agent(&sandbox.id);
    store.create_agent(agent.clone()).await.unwrap();
    let first = store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::Stopped {
                reason: "first".into(),
            },
        )
        .await
        .unwrap();
    let second = store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::Stopped {
                reason: "second".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!((first.seq, second.seq), (1, 2));
    assert_eq!(
        store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .last_seq,
        2
    );
    let orphan = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 3,
        appended_at: NOW.into(),
        kind: AgentLogKind::Deleted,
    };
    store
        .agent_log
        .insert_one(log_document(&orphan).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        store.agent_log(&agent.id, 0, 10).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(matches!(
        store.agent_log(&agent.id, 2, 10).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    store
        .agent_log
        .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 3_i64 })
        .await
        .unwrap();
    // Restoring the orphan does not clear quarantine. Reset this synthetic
    // fixture only after auditing the restored prefix, before the gap case.
    super::agent_log_read_tests::reset_repaired_prefix(&store, &agent.id, 2).await;
    store
        .agent_log
        .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 1_i64 })
        .await
        .unwrap();
    assert_eq!(
        store.agent_log(&agent.id, 0, 10).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert!(matches!(
        store.agent_log(&agent.id, 1, 10).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}
