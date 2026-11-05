use super::*;

fn entries(agent: &Agent, kinds: Vec<AgentLogKind>) -> Vec<AgentLogEntry> {
    kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: index as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect()
}

fn created(agent: &Agent) -> AgentLogKind {
    AgentLogKind::Created {
        workspace_id: agent.workspace_id.clone(),
        worktree_id: agent.worktree_id.clone(),
    }
}

fn approval(id: usize) -> AgentLogKind {
    AgentLogKind::ApprovalRequested {
        approval_id: format!("approval-{id}"),
        action: Some("fixture".into()),
        detail: None,
        runtime_locator: None,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn indexed_approvals_page_without_loading_the_outstanding_set() {
    let (client, name) = database("segment_approvals").await;
    segments::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, agent) = worktree_and_agent(&SandboxId("sbx_segments".into()));
    let mut kinds = vec![created(&agent)];
    kinds.extend((0..250).map(approval));
    kinds.push(AgentLogKind::ApprovalResolved {
        approval_id: "approval-1".into(),
        decision: "approved".into(),
    });
    let source = entries(&agent, kinds);
    let (agent, source) = install(&store, agent, source, 125).await;
    let (found, other) = store
        .pending_agent_approval(&agent.id, "approval-240")
        .await
        .unwrap();
    assert_eq!(found, Some(source[241].clone()));
    assert!(other);
    assert_eq!(
        store
            .pending_agent_approval(&agent.id, "approval-1")
            .await
            .unwrap(),
        (None, true)
    );
    assert!(matches!(
        store.pending_agent_approvals(&agent.id).await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    let first = store
        .pending_agent_approvals_page(&agent.id, 252, 0, 200)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 200);
    let second = store
        .pending_agent_approvals_page(&agent.id, 252, first.next_after.unwrap(), 200)
        .await
        .unwrap();
    assert_eq!(second.items.len(), 49);
    assert_eq!(second.next_after, None);
    let all = first
        .items
        .into_iter()
        .chain(second.items)
        .collect::<Vec<_>>();
    assert_eq!(
        all,
        source[1..251]
            .iter()
            .filter(|entry| entry.seq != 3)
            .cloned()
            .collect::<Vec<_>>()
    );
    assert!(matches!(
        store
            .pending_agent_approvals_page(&agent.id, 251, 0, 200)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert!(matches!(
        store
            .pending_agent_approvals_page(&agent.id, 252, 0, 201)
            .await,
        Err(SandboxStoreError::InvalidPage)
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn indexed_answer_context_keeps_exact_original_turn_and_terminal_evidence() {
    let (client, name) = database("segment_answer").await;
    segments::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, mut agent) = worktree_and_agent(&SandboxId("sbx_segments".into()));
    agent.conversation.conversation_id = Some("native-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    let source = entries(
        &agent,
        vec![
            created(&agent),
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
            AgentLogKind::AgentMessage {
                content: "fixture output".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
            AgentLogKind::ModeChanged {
                mode: "fixture".into(),
            },
            AgentLogKind::ModeChanged {
                mode: "fixture".into(),
            },
        ],
    );
    let (agent, _) = install(&store, agent, source, 3).await;
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let actual = segments::answer::context(&store, &agent, &mut session)
        .await
        .unwrap();
    assert_eq!(
        actual,
        Some(Some(layer2_core::sandbox::agent_io::AnswerContext {
            turn_sequence: 3,
            answer_sequence: 5
        }))
    );
    session.abort_transaction().await.unwrap();
    let answer = store.agent_log_final_answer(&agent, 5).await.unwrap();
    let layer2_core::sandbox::AgentLogAnswerLookup::Answer(answer) = answer else {
        panic!("exact completed answer must remain available after later appends");
    };
    assert_eq!(answer.start.seq, 3);
    assert_eq!(answer.message.seq, 5);
    assert_eq!(answer.terminal.seq, 6);
    let layer2_core::sandbox::AgentLogTurnLookup::Active { start, .. } =
        store.agent_log_turn_at(&agent, 4).await.unwrap()
    else {
        panic!("a mid-turn window must retain the original turn start");
    };
    assert_eq!(start.seq, 3);
    assert!(matches!(
        store.agent_log_turn_at(&agent, 6).await.unwrap(),
        layer2_core::sandbox::AgentLogTurnLookup::Inactive
    ));
    assert!(matches!(
        store.agent_log_final_answer(&agent, 4).await.unwrap(),
        layer2_core::sandbox::AgentLogAnswerLookup::NotFinal
    ));
    let evidence_filter = doc! {"agent_id":agent.id.as_str(),"kind":"turn","entity_key":"3","evidence_generation":"initial"};
    let evidence = store
        .segment_logs
        .evidence
        .find_one(evidence_filter.clone())
        .await
        .unwrap()
        .unwrap();
    store
        .segment_logs
        .evidence
        .delete_one(evidence_filter.clone())
        .await
        .unwrap();
    assert!(
        store.agent_log_turn_at(&agent, 4).await.is_err(),
        "a missing indexed turn must not become an inactive context"
    );
    start_transaction(&mut session).await.unwrap();
    assert!(
        segments::title_transcript(&store, &agent, &mut session, false)
            .await
            .is_err(),
        "a missing first-turn row must not select a later turn"
    );
    session.abort_transaction().await.unwrap();
    store
        .segment_logs
        .evidence
        .insert_one(evidence)
        .await
        .unwrap();
    // An indexed reference alone cannot certify changed original bytes.
    store
        .agent_log
        .update_one(
            doc! {"agent_id":agent.id.as_str(),"seq":5_i64},
            doc! {"$set":{"record.kind.content":"changed"}},
        )
        .await
        .unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(segments::answer::context(&store, &agent, &mut session)
        .await
        .is_err());
    session.abort_transaction().await.unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn segmented_projection_missing_stale_or_unknown_never_falls_back() {
    let (client, name) = database("segment_projection_shape").await;
    segments::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 3).await;
    assert_eq!(
        store
            .agent_log_projection(&agent)
            .await
            .unwrap()
            .unwrap()
            .through_seq,
        8
    );
    let filter = doc! {"_id":agent.id.as_str()};
    let original = store
        .segment_logs
        .projections
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    store
        .segment_logs
        .projections
        .delete_one(filter.clone())
        .await
        .unwrap();
    assert!(matches!(
        store.agent_log_projection(&agent).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    store
        .segment_logs
        .projections
        .insert_one(original.clone())
        .await
        .unwrap();
    for fields in [
        doc! {"format_version":2_i64},
        doc! {"through_seq":9_i64},
        doc! {"through_seq":7_i64},
        doc! {"head_revision":2_i64},
        doc! {"record.latest_turn":9_i64},
    ] {
        store
            .segment_logs
            .projections
            .update_one(filter.clone(), doc! {"$set":fields})
            .bypass_document_validation(true)
            .await
            .unwrap();
        assert!(matches!(
            store.agent_log_projection(&agent).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        store
            .segment_logs
            .projections
            .replace_one(filter.clone(), original.clone())
            .await
            .unwrap();
    }
    assert_eq!(
        store
            .agent_log_projection(&agent)
            .await
            .unwrap()
            .unwrap()
            .through_seq,
        8
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn indexed_title_context_preserves_first_turn_policy_for_steering_and_gaps() {
    for steering in [false, true] {
        let (client, name) = database("segment_title_policy").await;
        segments::migrate(&client.database(&name)).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (_, mut agent) = worktree_and_agent(&SandboxId("sbx_segments".into()));
        agent.conversation.conversation_id = Some("native-fixture".into());
        agent.conversation.runtime_generation = Some(1);
        let mut kinds = vec![
            created(&agent),
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
            AgentLogKind::AgentMessage {
                content: "fixture output".into(),
            },
        ];
        if steering {
            kinds.extend([
                AgentLogKind::OperationDeliveryChanged {
                    operation_sequence: 6,
                    operation: layer2_core::sandbox::AgentOperationKind::Steer,
                    state: layer2_core::sandbox::AgentOperationDelivery::DispatchFenced,
                },
                AgentLogKind::OwnerMessage {
                    content: "fixture steering".into(),
                },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ]);
        } else {
            kinds.extend([
                AgentLogKind::ConversationGap { frames: 1 },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
                AgentLogKind::ModeChanged {
                    mode: "fixture".into(),
                },
            ]);
        }
        let source = entries(&agent, kinds);
        let (agent, source) = install(&store, agent, source, 3).await;
        let expected = layer2_core::sandbox::AgentTitleTranscript::from_agent_log(&agent, &source);
        assert_eq!(expected.is_none(), steering);
        let mut session = store.client.start_session().snapshot(true).await.unwrap();
        let actual = segments::title_transcript(&store, &agent, &mut session, true)
            .await
            .unwrap();
        assert_eq!(actual, Some(expected));
        client.database(&name).drop().await.unwrap();
    }
}
