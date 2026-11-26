use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::store::{
    validate_agent_conversation_admission, AgentConversationMutation, SandboxStoreError,
};
use layer2_core::sandbox::Agent;
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::ApplyError;

pub(super) async fn require_current(
    mutation: &AgentConversationMutation,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    if mutation
        .settings_fence
        .as_ref()
        .is_some_and(|fence| fence.require_current().is_err())
        || mutation
            .execution_fence
            .as_ref()
            .is_some_and(|fence| fence.require_current().is_err())
        || mutation
            .trigger_execution_fence
            .as_ref()
            .is_some_and(|fence| fence.require_current().is_err())
    {
        let _ = session.abort_guarded().await;
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

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
    if let Some(fence) = &mutation.settings_fence {
        use super::super::resource_access::{read_execution_snapshot, ReadError};
        let snapshot = read_execution_snapshot(store, fence.target(), session)
            .await
            .map_err(|error| match error {
                ReadError::Driver(error) => ApplyError::Driver(error),
                ReadError::Domain(_) => ApplyError::Domain(SandboxStoreError::Conflict),
            })?;
        fence
            .validate_snapshot(&snapshot)
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    } else if let Some(fence) = &mutation.execution_fence {
        store
            .bindings
            .conversation_authority(fence.binding_authority(), session)
            .await?;
        use super::super::resource_access::{read_execution_snapshot, ReadError};
        let snapshot = read_execution_snapshot(store, &fence.target(), session)
            .await
            .map_err(|error| match error {
                ReadError::Driver(error) => ApplyError::Driver(error),
                ReadError::Domain(_) => ApplyError::Domain(SandboxStoreError::Conflict),
            })?;
        fence
            .validate_snapshot(&snapshot)
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    } else if mutation.command_claim.is_some() && mutation.trigger_execution_fence.is_none() {
        let policy = store
            .resource_policies
            .find_one(doc! { "_id": parent.id.as_str() })
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        if let Some(document) = policy {
            let personal = super::super::resource_access::read_inventory_policy(&document)
                .and_then(|policy| policy.personal_owner_of(&parent))
                .map_err(|error| {
                    ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string()))
                })?;
            if !personal {
                return Err(ApplyError::Domain(SandboxStoreError::Conflict));
            }
        }
    }
    let expected_revision = parent.revision;
    let mut fields = None;
    if parent
        .cancel_recovery_pause(&mutation.appended_at)
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?
    {
        fields = Some(sandbox_replacement_fields(&parent).map_err(ApplyError::Domain)?);
    }
    // A real parent write serializes phase/owner-kind checks with maintenance,
    // lifecycle and resource-policy commits (including an absent sidecar).
    let locked = crate::store::work_lease_update::with_fence(
        &store.sandboxes,
        doc! {
            "_id": parent.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(expected_revision, "Agent admission Sandbox revision")
                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        },
        fields,
        "agent_conversation_admission_fence",
        session,
    )
    .await
    .map_err(ApplyError::Driver)?;
    if locked.matched_count == 1 {
        crate::store::sandbox_usage::save::record_saved(store, session, &parent, expected_revision)
            .await
            .map_err(|error| match error {
                crate::store::sandbox_events::TransactionEventError::Domain(error) => {
                    ApplyError::Domain(error)
                }
                crate::store::sandbox_events::TransactionEventError::Driver(error) => {
                    ApplyError::Driver(error)
                }
            })?;
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}
