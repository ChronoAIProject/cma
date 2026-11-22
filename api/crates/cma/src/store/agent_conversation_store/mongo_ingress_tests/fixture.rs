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
    pub(super) parent: Sandbox,
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

pub(super) fn projection_fixture(
    agent: &Agent,
    count: usize,
) -> (
    Agent,
    crate::store::agent_log_segments::Head,
    crate::store::agent_log_segments::projection::Projection,
) {
    use crate::store::agent_log_segments::projection::Projection;
    use layer2_core::sandbox::{
        AgentLogProjectionState, AgentLogQueuedReference, QueuedAgentMessage,
    };
    let mut agent = agent.clone();
    agent.conversation.last_seq = 128;
    agent.conversation.active_turn = None;
    agent.conversation.conversation_id = None;
    agent.conversation.runtime_generation = None;
    agent.conversation.queued_messages = (0..count)
        .map(|n| {
            QueuedAgentMessage::new(
                n as u64 + 1,
                format!("queued {n}"),
                "2026-01-01T00:00:00Z".into(),
            )
            .unwrap()
        })
        .collect();
    let mut record = AgentLogProjectionState::default();
    record.through_seq = 128;
    record.latest_task_status = Some(1);
    record.unconsumed_queue = agent
        .conversation
        .queued_messages
        .iter()
        .map(|queued| AgentLogQueuedReference {
            sequence: queued.sequence,
            payload_sha256: queued.payload_sha256.clone(),
        })
        .collect();
    record.validate_aggregate(&agent).unwrap();
    let (mut head, _) = super::read_ports::seal_fixture(128);
    head.agent_id = agent.id.as_str().into();
    let projection = Projection {
        agent_id: head.agent_id.clone(),
        format_version: 1,
        incarnation: head.incarnation.clone(),
        evidence_generation: head.evidence_generation.clone(),
        through_seq: head.last_seq,
        head_revision: head.revision,
        record,
    };
    (agent, head, projection)
}

impl Fixture {
    pub(super) async fn prepare_reopen(&mut self) -> layer2_core::sandbox::store::AgentReopenGuard {
        use layer2_core::sandbox::{
            SandboxCheckpointConversation, SandboxCheckpointRef, SandboxImageRotationPhase,
            SandboxLifecycleOperation, SandboxRotationIdentity,
        };
        use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
        use sha2::{Digest, Sha256};
        let at = layer2_core::now_rfc3339();
        let native = self.agent.conversation.conversation_id.clone().unwrap();
        let archive = b"synthetic reopen archive".to_vec();
        let conversation = SandboxCheckpointConversation::new(
            self.agent.id.clone(),
            native.clone(),
            ConversationSnapshot {
                conversation_id: native.clone(),
                archive_sha256: format!("{:x}", Sha256::digest(&archive)),
                archive,
            },
        )
        .unwrap();
        let checkpoint = SandboxCheckpointRef::seal(
            &CheckpointReceipt {
                epoch: 1,
                object_reference: "synthetic/reopen".into(),
                sha256: "e".repeat(64),
                writers: WriterState::Resumed,
            },
            &[conversation],
            &at,
        )
        .unwrap();
        self.parent
            .lifecycle
            .checkpoints
            .publish(checkpoint)
            .unwrap();
        let identity =
            SandboxRotationIdentity::capture(&self.parent, std::slice::from_ref(&self.agent))
                .unwrap();
        self.parent
            .begin_image_rotation("image-v2".into(), identity, at.clone())
            .unwrap();
        let Some(SandboxLifecycleOperation::ImageRotation {
            phase,
            checkpoint_epoch,
            ..
        }) = self.parent.lifecycle.operation.as_mut()
        else {
            unreachable!()
        };
        *phase = SandboxImageRotationPhase::ConversationReopen;
        *checkpoint_epoch = Some(1);
        self.parent.runtime.as_mut().unwrap().provision_generation = Some(2);
        self.parent.runtime.as_mut().unwrap().runtime_version = "image-v2".into();
        self.parent.lifecycle.runtime_generation_high_water = 2;
        self.parent
            .work
            .claim(
                "synthetic-reopen-worker".into(),
                &at,
                layer2_core::rfc3339_at(
                    std::time::SystemTime::now() + std::time::Duration::from_secs(300),
                ),
            )
            .unwrap();
        self.parent.validate().unwrap();
        self.agent.status = AgentStatus::Paused;
        self.store.sandboxes.update_one(mongodb::bson::doc! {"_id":self.parent.id.as_str()}, mongodb::bson::doc! {
            "$set": crate::store::sandboxes::sandbox_replacement_fields(&self.parent).unwrap(),
        }).await.unwrap();
        self.store
            .agents
            .update_one(
                mongodb::bson::doc! {"_id":self.agent.id.as_str()},
                mongodb::bson::doc! {
                    "$set":crate::store::agents::agent_replacement_fields(&self.agent).unwrap(),
                },
            )
            .await
            .unwrap();
        layer2_core::sandbox::store::AgentReopenGuard {
            sandbox: SandboxMutationGuard::capture(&self.parent),
            agent_fence: self.agent.work.next_fence,
            native_conversation_id: native,
            runtime_generation: 2,
        }
    }
}
