use layer2_core::ids::SubjectId;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore, SandboxEventStore, SandboxStore};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentOperationDelivery, AgentOperationIntent, AgentOperationRecord,
    AgentStatus, AgentStopReason, AgentWorkStep, RuntimeBinding, SandboxCheckpointConversation,
    SandboxCheckpointRef, SandboxEventKind, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxPausePhase, SandboxReviveCause, SandboxRotationIdentity, SandboxStatus, SandboxWorkStep,
    TypedFailure,
};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use mongodb::options::ClientOptions;
use mongodb::Client;
use sha2::{Digest, Sha256};

use super::*;
use crate::store::agents::agent_replacement_fields;
use crate::store::sandbox_maintenance::mongo_integration_tests::{
    active_sandbox, database, fixture, insert_agent, ready_agent,
};
use crate::store::sandbox_mongo::start_transaction;
use crate::store::sandboxes::{sandbox_document, sandbox_replacement_fields};

const BEFORE: &str = "2026-09-05T00:00:00Z";
const OBSERVED: &str = "2026-09-05T00:30:00Z";

mod authority;
mod bounded;
mod channel_home_retention;
mod commit;
mod recovery;
mod recovery_overlap;
mod refusal;
mod retirement;
mod retirement_commit;
mod retirement_custody;

mod scoped;

fn loss(sandbox: &Sandbox) -> SandboxRuntimeLossRequest {
    SandboxRuntimeLossRequest {
        sandbox_id: sandbox.id.clone(),
        expected_revision: sandbox.revision,
        expected_runtime: sandbox.runtime.clone(),
        native_retirement_finality: false,
        retain_runtime: false,
        interruption_cause: Default::default(),
        observed_at: OBSERVED.into(),
    }
}

fn checkpoint(agents: &[Agent]) -> SandboxCheckpointRef {
    let conversations = agents
        .iter()
        .map(|agent| {
            let conversation_id = agent.conversation.conversation_id.clone().unwrap();
            let archive = format!("conversation archive for {}", agent.id).into_bytes();
            SandboxCheckpointConversation::new(
                agent.id.clone(),
                &conversation_id,
                ConversationSnapshot {
                    conversation_id: conversation_id.clone(),
                    archive_sha256: format!("{:x}", Sha256::digest(&archive)),
                    archive,
                },
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch: 1,
            object_reference: "checkpoint/runtime-loss/1".into(),
            sha256: "a".repeat(64),
            writers: WriterState::Held,
        },
        &conversations,
        BEFORE,
    )
    .unwrap()
}

async fn checkpoint_fixture(store: &MongoSandboxStore, id: &str) -> (Sandbox, Agent) {
    let (mut sandbox, agent) = fixture(store, id).await;
    sandbox.lifecycle.checkpoints.latest = Some(checkpoint(std::slice::from_ref(&agent)));
    let expected = sandbox.revision;
    sandbox.revision += 1;
    store.save_sandbox(expected, sandbox.clone()).await.unwrap();
    (sandbox, agent)
}

async fn disable_failpoint(client: &Client) {
    client
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("disable failpoint before assertions");
}
