use layer2_core::sandbox::store::{apply_agent_sign_in, AgentWorkMutation, SandboxStoreError};
use layer2_core::sandbox::{Agent, AgentLogKind, AgentStatus, Sandbox};
use mongodb::bson::doc;

use super::{ApplyError, MongoSandboxStore};
use crate::store::sandbox_mongo::{signed, DOCUMENT_SCHEMA_VERSION};
use crate::store::sandboxes::{read_sandbox, sandbox_replacement_fields};

pub(super) async fn apply(
    store: &MongoSandboxStore,
    current: &Agent,
    mutation: &AgentWorkMutation,
    session: &mut mongodb::ClientSession,
) -> Result<Option<Sandbox>, ApplyError> {
    if !(matches!(
        mutation.agent_logs.as_slice(),
        [AgentLogKind::SignInRequired { .. }]
    ) || (current.status == AgentStatus::Starting
        && mutation.agent.status == AgentStatus::Ready))
    {
        return Ok(None);
    }
    let sandbox = store
        .sandboxes
        .find_one(doc! { "_id": current.sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let next = apply_agent_sign_in(&sandbox, current, mutation).map_err(ApplyError::Domain)?;
    if next.is_none()
        && !matches!(
            mutation.agent_logs.as_slice(),
            [AgentLogKind::SignInRequired { .. }]
        )
    {
        return Ok(None);
    }
    let next = next.unwrap_or_else(|| sandbox.clone());
    // Repeated challenges still serialize with an owner pause of the parent.
    let written = store.sandboxes.update_one(
        doc! {
            "_id": sandbox.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(sandbox.revision, "sandbox sign-in revision").map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        },
        doc! {
            "$set": sandbox_replacement_fields(&next).map_err(ApplyError::Domain)?,
            "$inc": { "runtime_writer_claim_fence": 1_i64 },
        },
    ).session(&mut *session).await.map_err(ApplyError::Driver)?;
    if written.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    crate::store::sandbox_usage::save::record_saved(store, session, &next, sandbox.revision)
        .await?;
    Ok(Some(next))
}
