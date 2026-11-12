use super::*;
use mongodb::{bson, Client};
use segments::mongo_tests::fixtures;
pub(super) const NOW: &str = "2026-11-01T00:00:00Z";

pub(super) async fn fixture(
    closed: i64,
    dense: bool,
) -> (
    Client,
    mongodb::Database,
    MongoSandboxStore,
    Agent,
    Vec<AgentLogEntry>,
) {
    let (client, name) = fixtures::database("s3_transcript").await;
    let db = client.database(&name);
    segments::schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut agent = fixtures::agent();
    agent.conversation.conversation_id = Some("native".into());
    agent.conversation.runtime_generation = Some(1);
    let mut kinds = vec![
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
            model: Some("model".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "owner".into(),
        },
    ];
    while kinds.len() < (closed * 16 + 5) as usize {
        kinds.push(if dense {
            AgentLogKind::AgentMessage {
                content: "多字节 answer".into(),
            }
        } else {
            AgentLogKind::ModeChanged {
                mode: "x".repeat(512),
            }
        });
    }
    kinds.extend([
        AgentLogKind::AgentMessage {
            content: "last answer".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        AgentLogKind::Usage {
            input_tokens: 5,
            output_tokens: 8,
            context_used: 13,
            context_window: Some(200),
        },
    ]);
    let entries = kinds
        .into_iter()
        .enumerate()
        .map(|(n, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: n as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect();
    let (agent, entries) = fixtures::install(&store, agent, entries, closed, 16).await;
    certify(&store, &agent, &entries).await;
    (client, db, store, agent, entries)
}

pub(super) async fn certify(store: &MongoSandboxStore, agent: &Agent, entries: &[AgentLogEntry]) {
    let head = segments::Head::read(
        store
            .segment_logs
            .heads
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let mut state = layer2_core::sandbox::AgentLogProjectionState::default();
    let mut evidence = std::collections::BTreeMap::new();
    for entry in entries {
        for change in state
            .observe(
                agent,
                entry,
                layer2_core::sandbox::AgentLogProjectionState::queued_owner(entry),
            )
            .unwrap()
        {
            let row = projection::plan(&head, change).unwrap();
            evidence.insert(row.get_str("_id").unwrap().to_owned(), row);
        }
    }
    state.validate_aggregate(agent).unwrap();
    if !evidence.is_empty() {
        store
            .segment_logs
            .evidence
            .insert_many(evidence.into_values())
            .await
            .unwrap();
    }
    let row = projection::Projection {
        agent_id: head.agent_id,
        format_version: 1,
        incarnation: head.incarnation,
        evidence_generation: head.evidence_generation,
        through_seq: head.last_seq,
        head_revision: head.revision,
        record: state,
    };
    store
        .segment_logs
        .projections
        .insert_one(bson::serialize_to_document(&row).unwrap())
        .await
        .unwrap();
}

pub(super) fn expected(agent: &Agent, entries: &[AgentLogEntry]) -> AgentTranscript {
    let mut fold = layer2_core::sandbox::AgentTranscriptFold::default();
    for entry in entries {
        fold.push(agent, entry.clone()).unwrap();
    }
    fold.finish(agent).unwrap()
}
pub(super) fn first(limit: u16) -> layer2_core::sandbox::AgentTranscriptPageRequest {
    layer2_core::sandbox::AgentTranscriptPageRequest {
        turn: None,
        after: None,
        through: None,
        limit,
    }
}

pub(super) async fn collect(
    store: &MongoSandboxStore,
    agent: &Agent,
    limit: u16,
) -> AgentTranscript {
    let mut request = first(limit);
    let mut result = AgentTranscript {
        agent_id: agent.id.clone(),
        through_seq: agent.conversation.last_seq,
        turns: vec![],
    };
    loop {
        let mut session = store.client.start_session().snapshot(true).await.unwrap();
        let page = page(store, agent, &mut session, &request)
            .await
            .unwrap()
            .unwrap();
        for mut turn in page.transcript.turns {
            if let Some(previous) = result
                .turns
                .last_mut()
                .filter(|t| t.turn_sequence == turn.turn_sequence)
            {
                previous.messages.append(&mut turn.messages);
                previous.agent_messages.append(&mut turn.agent_messages);
            } else {
                result.turns.push(turn);
            }
        }
        let Some(next) = page.next else {
            return result;
        };
        assert!(next.after > request.after);
        request = next;
    }
}

pub(super) async fn custom(
    kinds: Vec<AgentLogKind>,
    generation: u64,
    active: Option<u64>,
) -> (
    mongodb::Database,
    MongoSandboxStore,
    Agent,
    Vec<AgentLogEntry>,
) {
    custom_with_setup(kinds, generation, active, false).await
}

pub(super) async fn custom_with_setup(
    kinds: Vec<AgentLogKind>,
    generation: u64,
    active: Option<u64>,
    setup: bool,
) -> (
    mongodb::Database,
    MongoSandboxStore,
    Agent,
    Vec<AgentLogEntry>,
) {
    let (client, name) = fixtures::database("s3_parity").await;
    let db = client.database(&name);
    segments::schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut agent = fixtures::agent();
    agent.conversation.conversation_id = Some("native".into());
    agent.conversation.runtime_generation = Some(generation);
    agent.conversation.active_turn = active;
    if setup {
        let mut first =
            layer2_core::sandbox::FirstMessageRecord::new("setup", "Prepare workspace", NOW)
                .unwrap();
        first.purpose = Some(layer2_core::sandbox::FirstMessagePurpose::Setup);
        first.fence_dispatch_for(3, 3, 1, NOW.into()).unwrap();
        first.complete(NOW.into()).unwrap();
        agent.conversation.first_message = Some(first);
    }

    let mut entries = vec![AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 1,
        appended_at: NOW.into(),
        kind: AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
    }];
    entries.extend(
        kinds
            .into_iter()
            .enumerate()
            .map(|(n, kind)| AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: n as u64 + 2,
                appended_at: NOW.into(),
                kind,
            }),
    );
    let closed = (entries.len() as i64 - 1) / 4;
    let (agent, entries) = fixtures::install(&store, agent, entries, closed, 4).await;
    certify(&store, &agent, &entries).await;
    (db, store, agent, entries)
}
