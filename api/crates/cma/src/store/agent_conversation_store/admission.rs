use layer2_core::sandbox::store::{
    validate_agent_conversation_admission, AgentConversationMutation, SandboxStoreError,
};
use layer2_core::sandbox::Agent;
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::ApplyError;

pub(super) async fn validate_and_fence(
    store: &MongoSandboxStore,
    current: &Agent,
    mutation: &AgentConversationMutation,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let parent = match mutation.admission_guard.as_ref() {
        Some(guard) => store
            .sandboxes
            .find_one(doc! { "_id": guard.sandbox_id.as_str() })
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .as_ref()
            .map(read_sandbox)
            .transpose()
            .map_err(ApplyError::Domain)?,
        None => None,
    };
    validate_agent_conversation_admission(current, parent.as_ref(), mutation)
        .map_err(ApplyError::Domain)?;
    let Some(mut parent) = parent else {
        return Ok(());
    };
    let expected_revision = parent.revision;
    let mut update = doc! { "$inc": { "agent_conversation_admission_fence": 1_i64 } };
    if parent
        .cancel_recovery_pause(&mutation.appended_at)
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?
    {
        update.insert(
            "$set",
            sandbox_replacement_fields(&parent).map_err(ApplyError::Domain)?,
        );
    }
    // A real parent write serializes the phase check with maintenance and lifecycle commits.
    let locked = store
        .sandboxes
        .update_one(
            doc! {
                "_id": parent.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": signed(expected_revision, "Agent admission Sandbox revision")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
            },
            update,
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if locked.matched_count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}
