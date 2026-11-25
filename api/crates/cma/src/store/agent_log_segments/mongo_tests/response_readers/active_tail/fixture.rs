use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct Case {
    pub name: &'static str,
    pub rows: i64,
    pub bytes: Option<i64>,
}

pub(super) const CASES: [Case; 4] = [
    Case {
        name: "100_rows",
        rows: 100,
        bytes: None,
    },
    Case {
        name: "1000_rows",
        rows: 1_000,
        bytes: None,
    },
    Case {
        name: "near_10000_rows",
        rows: 9_990,
        bytes: None,
    },
    Case {
        name: "near_64_mib",
        rows: 1_000,
        bytes: Some(64 * 1024 * 1024 - 16 * 1024),
    },
];
const SEALED: i64 = 256;

pub(super) struct Fixture {
    pub agent: Agent,
    pub target: Vec<AgentLogEntry>,
    pub active: Vec<AgentLogEntry>,
    pub active_bytes: i64,
}

fn row(agent: &Agent, seq: i64, kind: AgentLogKind) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: seq as u64,
        appended_at: NOW.into(),
        kind,
    }
}

fn mode(bytes: usize) -> AgentLogKind {
    AgentLogKind::ModeChanged {
        mode: "x".repeat(bytes),
    }
}

pub(super) async fn install(store: &MongoSandboxStore, case: Case) -> Fixture {
    let mut agent = agent();
    agent.status = layer2_core::sandbox::AgentStatus::Ready;
    agent.conversation.conversation_id = Some("active-tail-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    let mut rows = (1..=SEALED)
        .map(|seq| {
            let kind = match seq {
                1 => AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.worktree_id.clone(),
                },
                2 => AgentLogKind::ConversationOpened {
                    conversation_id: "active-tail-fixture".into(),
                    runtime_generation: 1,
                },
                3 => AgentLogKind::TurnStarted {
                    turn_sequence: 3,
                    origin_key: None,
                    model: Some("original-model".into()),
                },
                4 => AgentLogKind::OwnerMessage {
                    content: "fixed request".into(),
                },
                202 => AgentLogKind::AgentMessage {
                    content: "fixed answer".into(),
                },
                203 => AgentLogKind::TurnCompleted { turn_sequence: 3 },
                204 => AgentLogKind::Usage {
                    input_tokens: 7,
                    output_tokens: 11,
                    context_used: 18,
                    context_window: None,
                },
                _ => mode(8),
            };
            row(&agent, seq, kind)
        })
        .collect::<Vec<_>>();
    let empty = bson::serialize_to_vec(&log_document(&row(&agent, SEALED + 1, mode(0))).unwrap())
        .unwrap()
        .len() as i64;
    let payload = case
        .bytes
        .map(|bytes| bytes - empty * case.rows)
        .unwrap_or(32 * case.rows);
    assert!(payload >= 0);
    for offset in 0..case.rows {
        let size = payload / case.rows + i64::from(offset < payload % case.rows);
        rows.push(row(&agent, SEALED + offset + 1, mode(size as usize)));
    }
    let active_bytes = rows[SEALED as usize..]
        .iter()
        .map(|entry| {
            bson::serialize_to_vec(&log_document(entry).unwrap())
                .unwrap()
                .len() as i64
        })
        .sum();
    if let Some(wanted) = case.bytes {
        assert_eq!(active_bytes, wanted);
    }
    let (agent, rows) = super::super::super::fixtures::install(store, agent, rows, 1, SEALED).await;
    let db = store.client.database(&store.agent_log.namespace().db);
    super::super::fixtures::certify(&store.client, &db, &agent, 1).await;
    Fixture {
        agent,
        target: rows[..SEALED as usize].to_vec(),
        active: rows[SEALED as usize..].to_vec(),
        active_bytes,
    }
}
