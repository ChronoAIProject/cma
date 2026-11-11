use super::*;
use crate::store::agent_log::log_document;
use crate::store::agent_log_integrity::INTEGRITY_FIELD;
use crate::store::mongo_read_telemetry as telemetry;
use layer2_core::sandbox::store::AgentLogStore;

struct Fixture {
    client: Client,
    name: String,
    store: MongoSandboxStore,
    agent: Agent,
    entries: Vec<AgentLogEntry>,
}

impl Fixture {
    async fn new() -> Self {
        let (client, name) = database("agent_log_usage_projection").await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let parent = sandbox("sbx_usage_projection");
        store.create_sandbox(parent.clone()).await.unwrap();
        let (_, mut agent) = worktree_and_agent(&parent.id);
        agent.status = AgentStatus::Ready;
        agent.conversation.conversation_id = Some("usage-projection".into());
        agent.conversation.runtime_generation = Some(1);
        store.create_agent(agent.clone()).await.unwrap();
        let mut kinds = vec![
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.creation_worktree_id().clone(),
            },
            AgentLogKind::ConversationOpened {
                conversation_id: "usage-projection".into(),
                runtime_generation: 1,
            },
            AgentLogKind::Usage {
                input_tokens: 11,
                output_tokens: 7,
                context_used: 4,
                context_window: Some(32),
            },
        ];
        kinds.extend((0..997).map(|_| AgentLogKind::ModeChanged {
            mode: "idle".into(),
        }));
        kinds.push(AgentLogKind::UsageUpdated {
            input_tokens: 15,
            output_tokens: 9,
            context_used: 8,
            context_window: Some(64),
            turn_sequence: Some(3),
            turn_input_tokens: 5,
            turn_output_tokens: 3,
        });
        let entries = kinds
            .into_iter()
            .enumerate()
            .map(|(offset, kind)| AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: offset as u64 + 1,
                appended_at: NOW.into(),
                kind,
            })
            .collect::<Vec<_>>();
        for chunk in entries.chunks(200) {
            store
                .agent_log
                .insert_many(chunk.iter().map(|entry| log_document(entry).unwrap()))
                .await
                .unwrap();
        }
        agent.conversation.last_seq = entries.len() as u64;
        agent.revision += 1;
        let fields = crate::store::agents::agent_replacement_fields(&agent).unwrap();
        store
            .agents
            .update_one(
                doc! { "_id": agent.id.as_str() },
                doc! { "$set": fields, "$unset": { INTEGRITY_FIELD: "" } },
            )
            .await
            .unwrap();
        let store = store
            .prepare_bounded_admission(std::slice::from_ref(&agent.id))
            .await
            .unwrap()
            .with_bounded_usage_reads(true);
        Self {
            client,
            name,
            store,
            agent,
            entries,
        }
    }

    async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn trusted_usage_projection_selects_usage_rows_and_preserves_order() {
    let fixture = Fixture::new().await;
    let expected = fixture
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.kind,
                AgentLogKind::Created { .. }
                    | AgentLogKind::ConversationOpened { .. }
                    | AgentLogKind::Usage { .. }
                    | AgentLogKind::UsageUpdated { .. }
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    telemetry::isolated_test(async {
        let actual = fixture
            .store
            .agent_log_usage(&fixture.agent)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual, expected);
        assert!(telemetry::snapshot(AGENT_LOG)[2] < fixture.entries.len() as u64);
    })
    .await;
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn usage_projection_closes_without_trust_or_when_disabled() {
    let mut fixture = Fixture::new().await;
    fixture.store = fixture.store.clone().with_bounded_usage_reads(false);
    assert_eq!(
        fixture.store.agent_log_usage(&fixture.agent).await.unwrap(),
        None
    );
    fixture
        .store
        .agents
        .update_one(
            doc! { "_id": fixture.agent.id.as_str() },
            doc! { "$unset": { INTEGRITY_FIELD: "" } },
        )
        .await
        .unwrap();
    fixture.store = fixture.store.with_bounded_usage_reads(true);
    assert_eq!(
        fixture.store.agent_log_usage(&fixture.agent).await.unwrap(),
        None
    );
    fixture.cleanup().await;
}
