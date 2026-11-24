use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn root_fences_unchanged_terminal_segmented_child_with_nonempty_publication() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = adopted(&store, "sbx_terminal").await;
    let mut terminal = ready_agent(&sandbox, "agt_terminal_child");
    terminal
        .transition(
            AgentStatus::Stopped,
            BEFORE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    terminal.conversation.last_seq = 3;
    store
        .agents
        .insert_one(crate::store::agents::agent_document(&terminal).unwrap())
        .await
        .unwrap();
    for (index, kind) in [
        AgentLogKind::Created {
            workspace_id: terminal.workspace_id.clone(),
            worktree_id: terminal.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: terminal.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
        AgentLogKind::Stopped {
            reason: AgentStopReason::Owner.as_str().into(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        store
            .agent_log
            .insert_one(
                crate::store::agent_log::log_document(&AgentLogEntry {
                    agent_id: terminal.id.clone(),
                    seq: index as u64 + 1,
                    appended_at: BEFORE.into(),
                    kind,
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let terminal = admit(&store, &terminal, 1).await;
    let before = row(&store, &terminal).await;
    let logs = store.agent_log(&terminal.id, 0, 100).await.unwrap();
    assert_eq!(
        store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
        SandboxRuntimeLossOutcome::RecoveryScheduled
    );
    assert_eq!(
        store.get_agent(&terminal.id).await.unwrap(),
        Some(terminal.clone())
    );
    let mut after = row(&store, &terminal).await;
    assert_eq!(after.remove("sandbox_recovery_fence"), Some(1_i64.into()));
    assert_eq!(after, before);
    assert_eq!(store.agent_log(&terminal.id, 0, 100).await.unwrap(), logs);
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().status,
        AgentStatus::Paused
    );
    assert_eq!(publication_count(&store).await, 3); // two adoptions, one mutation
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn parent_cas_race_aborts_scoped_publication_then_public_root_refuses_stale_request() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = adopted(&store, "sbx_parent_race").await;
    let original = row(&store, &agent).await;
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    scope
        .find_one(&store.sandboxes, doc! {"_id":sandbox.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    // Fix the losing transaction's snapshot, then advance the parent outside it.
    let mut winner = sandbox.clone();
    winner.revision += 1;
    winner.name.push_str("-winner");
    store
        .save_sandbox(sandbox.revision, winner.clone())
        .await
        .unwrap();
    let attempted = transaction::apply_scoped(&store, &loss(&sandbox), None, scope).await;
    assert!(matches!(attempted, Err(ApplyError::Driver(error)) if is_transient(&error)));
    session.abort_transaction().await.unwrap();
    unchanged(&store, &winner, &agent, &original).await;
    assert_eq!(
        store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
        SandboxRuntimeLossOutcome::Superseded
    );
    unchanged(&store, &winner, &agent, &original).await;
    client.database(&name).drop().await.unwrap();
}
