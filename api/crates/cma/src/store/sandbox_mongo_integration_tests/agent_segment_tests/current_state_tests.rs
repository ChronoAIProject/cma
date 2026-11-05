use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn current_controls_keep_original_references_and_reject_missing_source_or_changed_capture() {
    let (client, name) = database("segment_controls").await;
    segments::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, mut agent) = worktree_and_agent(&SandboxId("sbx_segments".into()));
    agent.conversation.conversation_id = Some("native-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.active_turn = Some(3);
    let kinds = vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-fixture".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "fixture input".into(),
        },
        AgentLogKind::AvailableCommandsSet { commands: vec![] },
        AgentLogKind::ApprovalRequested {
            approval_id: "fixture-approval".into(),
            action: None,
            detail: None,
            runtime_locator: None,
        },
        AgentLogKind::PlanSet { entries: vec![] },
        AgentLogKind::RuntimeMetadataChanged {
            metadata: Default::default(),
        },
        AgentLogKind::ConversationGap { frames: 2 },
    ];
    let entries = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: index as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect();
    let (agent, source) = install(&store, agent, entries, 3).await;
    let current = store
        .agent_log_current_state(&agent, 0, 100)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        current.context,
        [3, 5, 7, 8, 9].map(|seq| source[seq - 1].clone())
    );
    assert_eq!(current.approvals.items, vec![source[5].clone()]);
    assert_eq!(current.approvals.next_after, None);
    assert!(matches!(store.agent_log_turn_at(&agent, 7).await.unwrap(),
        layer2_core::sandbox::AgentLogTurnLookup::Active { start, gap } if start.seq == 3 && gap.is_none()));
    assert!(matches!(store.agent_log_turn_at(&agent, 9).await.unwrap(),
        layer2_core::sandbox::AgentLogTurnLookup::Active { start, gap: Some(gap) } if start.seq == 3 && gap.seq == 9));
    let mut stale = agent.clone();
    stale.revision += 1;
    assert!(matches!(
        store.agent_log_current_state(&stale, 0, 100).await,
        Err(SandboxStoreError::Conflict)
    ));
    store
        .agent_log
        .delete_one(doc! {"agent_id":agent.id.as_str(),"seq":5_i64})
        .await
        .unwrap();
    assert!(store.agent_log_current_state(&agent, 0, 100).await.is_err());
    client.database(&name).drop().await.unwrap();
}
