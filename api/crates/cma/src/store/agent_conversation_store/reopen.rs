use layer2_core::sandbox::store::{AgentReopenGuard, SandboxStoreError};
use layer2_core::sandbox::Agent;
use mongodb::{bson::doc, ClientSession};

use super::{ApplyError, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};

pub(in crate::store) async fn validate_and_fence(
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
    if let Some(restart) = &guard.owner_restart {
        let revision = super::signed(agent.revision, "restart output Agent revision")
            .map_err(SandboxStoreError::Corrupt)
            .map_err(ApplyError::Domain)?;
        let fence = super::signed(restart.lease.fence, "restart output fence")
            .map_err(SandboxStoreError::Corrupt)
            .map_err(ApplyError::Domain)?;
        let locked = store.agents.update_one(
            doc! {
                "_id": agent.id.as_str(),
                "revision": revision,
                "work_lease_holder": &restart.lease.holder,
                "work_lease_fence": fence,
                "work_lease_expires_at": &restart.lease.expires_at,
                "control_restart_active_v1.operation_id": &restart.restart_operation,
                "control_restart_dispatch_v1.operation_id": &restart.open_operation,
                "control_restart_dispatch_v1.request_sha256": &restart.open_request_sha256,
                "$expr": crate::store::agent_control_transaction::restart_live_lease_expression(),
            },
            doc! {"$inc": {"agent_conversation_reopen_fence": 1_i64}},
        ).session(&mut *session).await.map_err(ApplyError::Driver)?;
        if locked.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    // A real write prevents lifecycle replacement/lease takeover from committing
    // concurrently with a receipt based on an obsolete transaction snapshot.
    let filter = crate::store::work_claim_authority::without_ledger_maintenance(doc! {
        "_id": parent.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": super::signed(parent.revision, "reopen parent revision")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
    });
    let locked = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(filter),
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
