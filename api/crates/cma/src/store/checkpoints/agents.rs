use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::sandbox::store::{
    validate_checkpoint_mutation, SandboxCheckpointMutation, SandboxStoreError,
};
use layer2_core::sandbox::Sandbox;
use mongodb::bson::doc;

use super::super::agent_log::scan_complete_log_with_session;
use super::super::agent_log_integrity::TransactionLogError;
use super::super::lifecycle_execution::load_sandbox_agents;
use super::super::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::PublishError;

pub(super) async fn validate_and_fence(
    store: &MongoSandboxStore,
    current: &Sandbox,
    mutation: &SandboxCheckpointMutation,
    session: &mut mongodb::ClientSession,
) -> Result<(), PublishError> {
    if mutation.nonblocking {
        // The model requires the exact durable capture, lease and verified
        // export receipt. Current live logs belong to the next snapshot.
        return validate_checkpoint_mutation(current, &[], mutation).map_err(PublishError::Domain);
    }
    let agents = load_sandbox_agents(store, &current.id, session).await?;
    let mut rows = Vec::with_capacity(agents.len());
    for agent in agents {
        let last_seq = if agent.status.is_live() {
            scan_complete_log_with_session(
                ScanCaller::Checkpoint,
                store,
                &agent.id,
                agent.conversation.last_seq,
                session,
                |_| Ok(()),
            )
            .await?
            .entry_count()
        } else {
            agent.conversation.last_seq
        };
        rows.push((agent, last_seq));
    }
    validate_checkpoint_mutation(current, &rows, mutation).map_err(PublishError::Domain)?;

    // Real writes serialize against Agent changes and log appends after our snapshot.
    for (agent, last_seq) in rows.iter().filter(|(agent, _)| agent.status.is_live()) {
        let revision = signed(agent.revision, "checkpoint agent revision")
            .map_err(|detail| PublishError::Domain(SandboxStoreError::Corrupt(detail)))?;
        let last_seq = signed(*last_seq, "checkpoint agent log sequence")
            .map_err(|detail| PublishError::Domain(SandboxStoreError::Corrupt(detail)))?;
        let result = store
            .agents
            .update_one(
                doc! {
                    "_id": agent.id.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": revision,
                    "last_seq": last_seq,
                },
                doc! { "$inc": { "checkpoint_publication_fence": 1_i64 } },
            )
            .session(&mut *session)
            .await
            .map_err(PublishError::Driver)?;
        if result.matched_count != 1 {
            return Err(PublishError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(())
}

impl From<TransactionLogError> for PublishError {
    fn from(error: TransactionLogError) -> Self {
        match error {
            TransactionLogError::Domain(error) => Self::Domain(error),
            TransactionLogError::Driver(error) => Self::Driver(error),
        }
    }
}
