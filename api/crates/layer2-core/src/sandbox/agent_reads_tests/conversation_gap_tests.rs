use super::*;

async fn gap_history(
    kinds: Vec<AgentLogKind>,
    active: Option<u64>,
) -> (Arc<InMemorySandboxStore>, Agent) {
    let store = Arc::new(InMemorySandboxStore::default());
    let current = insert_hierarchy(
        &store,
        "agt_gap",
        "wtr_gap",
        "wks_gap",
        "sbx_gap",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let current = replace_history(
        &store,
        current,
        kinds,
        Some("conversation-1"),
        Some(1),
        active,
    )
    .await;
    (store, current)
}

fn opened() -> AgentLogKind {
    AgentLogKind::ConversationOpened {
        conversation_id: "conversation-1".into(),
        runtime_generation: 1,
    }
}

fn start(turn_sequence: u64) -> AgentLogKind {
    AgentLogKind::TurnStarted {
        model: None,
        turn_sequence,
        origin_key: None,
    }
}

fn owner() -> AgentLogKind {
    AgentLogKind::OwnerMessage {
        content: "Question".into(),
    }
}

fn answer(content: &str) -> AgentLogKind {
    AgentLogKind::AgentMessage {
        content: content.into(),
    }
}

#[tokio::test]
async fn declared_gap_preserves_history_and_marks_only_affected_completion_incomplete() {
    let kinds = vec![
        opened(),
        start(3),
        owner(),
        answer("Earlier complete answer"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        start(7),
        owner(),
        AgentLogKind::ConversationGap { frames: 35 },
        answer("Retained first part"),
        AgentLogKind::ConversationGap { frames: 2 },
        answer("Retained final part"),
        AgentLogKind::TurnCompleted { turn_sequence: 7 },
        start(14),
        owner(),
        answer("Later complete answer"),
        AgentLogKind::TurnCompleted { turn_sequence: 14 },
    ];
    let (store, current) = gap_history(kinds.clone(), None).await;
    let reads = service(store);
    let transcript = reads
        .transcript(&caller("owner", true), &current.id)
        .await
        .unwrap();
    assert_eq!(transcript.turns.len(), 3);
    for index in [0, 2] {
        assert!(matches!(
            transcript.turns[index].outcome,
            AgentTurnOutcome::Completed { .. }
        ));
    }
    assert_eq!(
        transcript.turns[1].agent_messages,
        ["Retained first part", "Retained final part"]
    );
    let AgentTurnOutcome::Failed { at, failure } = &transcript.turns[1].outcome else {
        panic!("missing output must not be represented as a complete response");
    };
    assert_eq!(at, NOW);
    assert_eq!(failure.class, "conversation_output_gap");
    assert!(failure
        .detail
        .starts_with("37 runtime output frames were lost."));
    let events = reads
        .log(&caller("owner", true), &current.id, 0, 100)
        .await
        .unwrap();
    assert_eq!(
        events
            .into_iter()
            .skip(1)
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        kinds
    );
    reads
        .usage(&caller("owner", true), &current.id)
        .await
        .unwrap();
}

#[tokio::test]
async fn gap_does_not_invent_a_terminal_or_replace_an_actual_failure_or_cancellation() {
    let failure = TypedFailure {
        class: "provider_failure".into(),
        detail: "Original provider failure".into(),
    };
    for terminal in [
        None,
        Some(AgentLogKind::TurnFailed {
            turn_sequence: 3,
            failure: failure.clone(),
        }),
        Some(AgentLogKind::TurnInterrupted { turn_sequence: 3 }),
    ] {
        let mut kinds = vec![
            opened(),
            start(3),
            owner(),
            AgentLogKind::ConversationGap { frames: 35 },
            answer("Retained output"),
        ];
        kinds.extend(terminal.clone());
        let (store, current) = gap_history(kinds, terminal.is_none().then_some(3)).await;
        let transcript = service(store)
            .transcript(&caller("owner", true), &current.id)
            .await
            .unwrap();
        let turn = &transcript.turns[0];
        assert_eq!(turn.agent_messages, ["Retained output"]);
        match terminal {
            None => assert_eq!(turn.outcome, AgentTurnOutcome::Active),
            Some(AgentLogKind::TurnFailed { .. }) => assert_eq!(
                turn.outcome,
                AgentTurnOutcome::Failed {
                    at: NOW.into(),
                    failure: failure.clone()
                }
            ),
            _ => assert_eq!(
                turn.outcome,
                AgentTurnOutcome::Interrupted { at: NOW.into() }
            ),
        }
    }
}

#[tokio::test]
async fn idle_gap_remains_visible_without_failing_the_next_turn() {
    let (store, current) = gap_history(
        vec![
            opened(),
            AgentLogKind::ConversationGap { frames: 3 },
            start(4),
            owner(),
            answer("Complete answer"),
            AgentLogKind::TurnCompleted { turn_sequence: 4 },
        ],
        None,
    )
    .await;
    let transcript = service(store)
        .transcript(&caller("owner", true), &current.id)
        .await
        .unwrap();
    assert!(matches!(
        transcript.turns[0].outcome,
        AgentTurnOutcome::Completed { .. }
    ));
}

#[tokio::test]
async fn zero_and_overflowing_gap_counts_remain_corrupt() {
    for gaps in [
        vec![AgentLogKind::ConversationGap { frames: 0 }],
        vec![
            AgentLogKind::ConversationGap { frames: u64::MAX },
            AgentLogKind::ConversationGap { frames: 1 },
        ],
    ] {
        let mut kinds = vec![opened(), start(3), owner()];
        kinds.extend(gaps);
        let (store, current) = gap_history(kinds, Some(3)).await;
        assert!(matches!(
            service(store)
                .transcript(&caller("owner", true), &current.id)
                .await,
            Err(AgentReadError::Corrupt(_))
        ));
    }
}
