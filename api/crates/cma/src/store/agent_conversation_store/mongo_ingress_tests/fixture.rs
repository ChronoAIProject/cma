use layer2_core::sandbox::store::{
    AgentConversationMutation, AgentConversationStore, AgentMessageCheckpoint, SandboxMutationGuard,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentMessageAssembly, AgentStatus, AgentUpdateOutcome, Sandbox,
};
use mongodb::Client;

use crate::store::sandbox_mongo::MongoSandboxStore;

pub(super) struct Fixture {
    client: Client,
    pub(super) database: String,
    pub(super) store: MongoSandboxStore,
    pub(super) agent: Agent,
    parent: Sandbox,
}

impl Fixture {
    pub(super) async fn with_client(client: Client, suffix: &str) -> Self {
        let database = format!("cma_ingress_{}", uuid::Uuid::new_v4().simple());
        let store = MongoSandboxStore::open(&client, &database).await.unwrap();
        let (parent, agent) = crate::store::sandbox_maintenance::mongo_integration_tests::fixture(
            &store,
            &format!("sbx_ingress_{suffix}"),
        )
        .await;
        Self {
            client,
            database,
            store,
            agent,
            parent,
        }
    }

    pub(super) async fn new(suffix: &str) -> Self {
        use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};

        let (client, database, store) = database().await;
        let (parent, agent) = fixture(&store, &format!("sbx_ingress_{suffix}")).await;
        Self {
            client,
            database,
            store,
            agent,
            parent,
        }
    }

    pub(super) async fn retain_prefix(&mut self, bytes: usize) -> AgentMessageCheckpoint {
        let now = "2026-01-01T00:00:00Z";
        let mut next = self.agent.clone();
        next.transition(AgentStatus::Working, now.into(), None)
            .unwrap();
        next.revision = self.agent.revision + 1;
        let turn = self.agent.conversation.last_seq + 1;
        next.conversation.active_turn = Some(turn);
        next.conversation.last_seq = self.agent.conversation.last_seq + 2;
        self.store
            .commit_agent_conversation(AgentConversationMutation {
                settings_fence: None,
                command_claim: None,
                execution_fence: None,
                trigger_execution_fence: None,
                admission_guard: Some(SandboxMutationGuard::capture(&self.parent)),
                expected_agent_revision: self.agent.revision,
                expected_last_seq: self.agent.conversation.last_seq,
                runtime_generation: self.agent.conversation.runtime_generation.unwrap(),
                agent: next.clone(),
                appended_at: now.into(),
                agent_logs: vec![
                    AgentLogKind::TurnStarted {
                        model: None,
                        turn_sequence: turn,
                        origin_key: None,
                    },
                    AgentLogKind::OwnerMessage {
                        content: "synthetic input".into(),
                    },
                ],
            })
            .await
            .unwrap();
        self.agent = next;
        let mut assembly = serde_json::to_value(AgentMessageAssembly::default()).unwrap();
        assembly["message_id"] = serde_json::json!("synthetic-message");
        assembly["content"] = serde_json::json!("x".repeat(bytes));
        let checkpoint = AgentMessageCheckpoint {
            version: 1,
            update_id: uuid::Uuid::new_v4().to_string(),
            update_fingerprint: "a".repeat(64),
            native_conversation_id: self.agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: self.agent.conversation.runtime_generation.unwrap(),
            active_turn: self.agent.conversation.active_turn,
            after_seq: self.agent.conversation.last_seq,
            assembly: serde_json::from_value(assembly).unwrap(),
            outcome: AgentUpdateOutcome {
                appended: Vec::new(),
                turn_closed: false,
            },
            output: Some(layer3_adapter::RuntimeOutputSource {
                stream_id: uuid::Uuid::new_v4().to_string(),
                cursor: 1,
                native_conversation_id: self.agent.conversation.conversation_id.clone().unwrap(),
                payload_sha256: "b".repeat(64),
            }),
        };
        self.store
            .commit_agent_message_update(self.agent.clone(), None, checkpoint.clone(), None)
            .await
            .unwrap();
        checkpoint
    }

    pub(super) async fn finish(self) {
        self.client.database(&self.database).drop().await.unwrap();
        drop(self.store);
        tokio::time::timeout(std::time::Duration::from_secs(5), self.client.shutdown())
            .await
            .expect("fixture Mongo resources did not finish cleanup");
    }
}
