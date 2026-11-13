use super::*;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn large_message_pages_shorten_without_losing_or_repeating_source_rows() {
    let mut kinds = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: "native".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "owner\n\"多".repeat(2048),
        },
    ];
    for index in 0..24 {
        kinds.push(AgentLogKind::AgentMessage {
            content: format!("{index}:{}", "message\n\"多".repeat(24 * 1024)),
        });
    }
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let (db, store, agent, entries) = custom(kinds, 1, None).await;
    assert!(serde_json::to_vec(&entries).unwrap().len() > MAX_BYTES);
    assert!(matches!(
        store.agent_log_transcript_projection(&agent).await,
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    ));
    let mut request = first(32);
    let mut sequences = Vec::new();
    let mut pages = 0;
    loop {
        let page = store
            .agent_log_transcript_page(&agent, &request)
            .await
            .unwrap()
            .unwrap();
        let turn = &page.transcript.turns[0];
        assert!(serde_json::to_vec(turn).unwrap().len() <= MAX_BYTES);
        sequences.extend(turn.messages.iter().map(|message| message.sequence));
        pages += 1;
        let Some(next) = page.next else { break };
        assert!(next.after > request.after);
        request = next;
    }
    assert!(pages > 1);
    let expected = expected(&agent, &entries);
    assert_eq!(
        sequences,
        expected.turns[0]
            .messages
            .iter()
            .map(|message| message.sequence)
            .collect::<Vec<_>>()
    );
    assert_eq!(collect(&store, &agent, 32).await, expected);
    let mut after = 0;
    let mut events = Vec::new();
    loop {
        let page = store
            .agent_log_page_projection(&agent, after, 200)
            .await
            .unwrap()
            .unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_BYTES);
        let Some(last) = page.last() else { break };
        assert!(last.seq > after);
        after = last.seq;
        events.extend(page);
    }
    assert_eq!(events, entries);
    db.drop().await.unwrap();
}

#[test]
fn transcript_byte_admission_accounts_for_escaping_and_both_message_fields() {
    use layer2_core::sandbox::{AgentLogProjectionState, AgentTurnOutcome};
    let agent = segments::mongo_tests::fixtures::agent();
    let mut state = AgentLogProjectionState::default();
    let kinds = [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "owner".into(),
        },
    ];
    for (index, kind) in kinds.into_iter().enumerate() {
        let entry = AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: index as u64 + 1,
            appended_at: NOW.into(),
            kind,
        };
        state.observe(&agent, &entry, None).unwrap();
    }
    let facts = state.active.unwrap();
    let empty = AgentTranscriptTurn {
        turn_sequence: 3,
        started_at: NOW.into(),
        origin_key: None,
        owner_message: "owner".into(),
        agent_messages: vec![],
        messages: vec![],
        metadata: Default::default(),
        outcome: AgentTurnOutcome::Active,
    };
    let entries = (5..=7)
        .map(|seq| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq,
            appended_at: NOW.into(),
            kind: AgentLogKind::AgentMessage {
                content: "\"多\n\\".repeat(20),
            },
        })
        .collect::<Vec<_>>();
    let mut full = empty.clone();
    assert_eq!(
        materialize::append(&facts, &mut full, entries.clone(), MAX_BYTES).unwrap(),
        3
    );
    let exact = serde_json::to_vec(&full).unwrap().len();
    let mut at_limit = empty.clone();
    assert_eq!(
        materialize::append(&facts, &mut at_limit, entries.clone(), exact).unwrap(),
        3
    );
    assert_eq!(at_limit, full);
    let mut limited = empty.clone();
    assert_eq!(
        materialize::append(&facts, &mut limited, entries, exact - 1).unwrap(),
        2
    );
    assert_eq!(limited.messages, full.messages[..2]);
    assert!(materialize::append(&facts, &mut empty.clone(), vec![], 0).is_err());
}
