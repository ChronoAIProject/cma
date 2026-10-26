use layer2_core::sandbox::store::{AgentReopenGuard, SandboxStoreError};
use layer2_core::sandbox::Agent;
use mongodb::{bson::doc, ClientSession};

use super::{ApplyError, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};

pub(super) async fn validate_and_fence(
    store: &MongoSandboxStore,
    agent: &Agent,
    guard: &AgentReopenGuard,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let row = store
        .sandboxes
        .find_one(doc! { "_id": agent.sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    let parent = super::super::sandboxes::read_sandbox(&row).map_err(ApplyError::Domain)?;
    guard
        .validate(&parent, agent, &layer2_core::now_rfc3339())
        .map_err(|_| {
            ApplyError::Domain(SandboxStoreError::InvalidTransaction(
                "reopen output authority is stale",
            ))
        })?;
    // A real write prevents lifecycle replacement/lease takeover from committing
    // concurrently with a receipt based on an obsolete transaction snapshot.
    let locked = store
        .sandboxes
        .update_one(
            doc! { "_id": parent.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": super::signed(parent.revision, "reopen parent revision")
                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))? },
            doc! { "$inc": { "agent_conversation_reopen_fence": 1_i64 } },
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if locked.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}
