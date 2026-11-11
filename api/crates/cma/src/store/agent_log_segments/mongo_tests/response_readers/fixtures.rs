use super::*;

pub(super) async fn source(
    store: &MongoSandboxStore,
    closed: i64,
    dense: bool,
) -> (Agent, Vec<AgentLogEntry>) {
    let mut agent = agent();
    agent.conversation.conversation_id = Some("native-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    let mut kinds = vec![
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
            model: Some("original-model".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "fixture".into(),
        },
    ];
    for _ in kinds.len()..(closed * 16 + 5) as usize {
        kinds.push(if dense {
            AgentLogKind::AgentMessage {
                content: "answer".into(),
            }
        } else {
            AgentLogKind::ModeChanged {
                mode: "x".repeat(512),
            }
        });
    }
    kinds.extend([
        AgentLogKind::AgentMessage {
            content: "final answer".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        AgentLogKind::Usage {
            input_tokens: 7,
            output_tokens: 11,
            context_used: 18,
            context_window: None,
        },
    ]);
    let entries = kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: i as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect();
    let (agent, entries) = super::super::fixtures::install(store, agent, entries, closed, 16).await;
    (agent, entries)
}

pub(super) async fn certify(client: &Client, db: &mongodb::Database, agent: &Agent, units: i64) {
    for _ in 0..units {
        assert_eq!(
            audit::run_unit(client, db, agent.id.as_str(), "response-reader", false)
                .await
                .unwrap(),
            "running"
        );
    }
    assert_eq!(
        audit::run_unit(client, db, agent.id.as_str(), "response-reader", false)
            .await
            .unwrap(),
        "complete"
    );
    assert_eq!(
        audit::install(client, db, agent.id.as_str(), "response-reader")
            .await
            .unwrap(),
        "installed"
    );
}
