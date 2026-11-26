//! Commit explicit scope expansion and its workflow receipt atomically.
use super::*;
use crate::store::sandbox_events::{
    append_in_transaction, validate_in_transaction, TransactionEventError,
};
use crate::store::sandbox_mongo::{signed, unsigned};
use crate::store::sandboxes::{read_sandbox, sandbox_replacement_fields};
use layer2_core::sandbox::{SandboxEventKind, SandboxStatus, SandboxWorkStep};

impl MongoSandboxStore {
    pub(crate) async fn grant_access_request_services(
        &self,
        lease: &AgentAccessLease,
    ) -> Result<AgentAccessRecord, AgentAccessError> {
        let at = layer2_core::now_rfc3339();
        transaction!(self, session, { apply_at(self, lease, &at, session).await })
    }
}

#[cfg(test)]
async fn apply(
    store: &MongoSandboxStore,
    lease: &AgentAccessLease,
    session: &mut ClientSession,
) -> TxResult<AgentAccessRecord> {
    apply_at(store, lease, &layer2_core::now_rfc3339(), session).await
}

async fn apply_at(
    store: &MongoSandboxStore,
    lease: &AgentAccessLease,
    at: &str,
    session: &mut ClientSession,
) -> TxResult<AgentAccessRecord> {
    let mut record = super::worker::leased(store, lease, session).await?;
    if !matches!(
        record.phase,
        AgentAccessPhase::Granting | AgentAccessPhase::Reviving
    ) {
        return Err(AgentAccessError::Conflict.into());
    }
    let snapshot = validate_lineage_transaction(store, &record.requester, session, false).await?;
    let value = store
        .sandboxes
        .find_one(doc! {"_id":record.requester.sandbox_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(AgentAccessError::NotFound)?;
    let current = read_sandbox(&value).map_err(|_| AgentAccessError::Unavailable)?;
    let operation_id = format!(
        "sandbox-lifecycle-v1:{}:{}",
        current.id.as_str(),
        current.lifecycle.next_authority.saturating_sub(1)
    );
    if current.status != SandboxStatus::Paused
        || current.runtime.is_some()
        || current.work.step != SandboxWorkStep::None
        || current.work.lease.is_some()
        || current.lifecycle.operation.is_some()
        || current.lifecycle.checkpoint_capture.is_some()
        || current.credential.pending.is_some()
        || record.progress.pause_operation_id.as_deref() != Some(&operation_id)
        || record.progress.revive_operation_id.is_some()
    {
        return Err(AgentAccessError::Conflict.into());
    }
    let services = record.approved_services()?.to_vec();
    let effective = current.credential.effective_reaches();
    if !effective.iter().all(|service| services.contains(service)) {
        return Err(AgentAccessError::Conflict.into());
    }
    if record.phase == AgentAccessPhase::Reviving {
        if !services.iter().all(|service| effective.contains(service))
            || record.progress.grant_generation_floor
                != Some(current.credential.service_access_generation_floor())
        {
            return Err(AgentAccessError::Conflict.into());
        }
        return Ok(record);
    }
    let last_seq = unsigned(
        value
            .get_i64("last_seq")
            .map_err(|_| AgentAccessError::Unavailable)?,
        "access grant event sequence",
    )
    .map_err(|_| AgentAccessError::Unavailable)?;
    validate_in_transaction(store, &current.id, last_seq, session)
        .await
        .map_err(event_error)?;
    let at = at.to_owned();
    let mut sandbox = current.clone();
    let grant = sandbox
        .grant_service_access(&record.requester.owner, &services, at.clone())
        .map_err(|_| AgentAccessError::Conflict)?;
    fence(store, &snapshot, session).await?;
    if let Some(grant) = grant {
        let event = SandboxEventKind::ServiceAccessGranted { grant };
        current
            .validate_service_access_transaction(&sandbox, &event, &at)
            .map_err(|_| AgentAccessError::Conflict)?;
        let result = store.sandboxes.update_one(crate::store::work_schema::query(doc! {
            "_id":current.id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "channel_reuse_cleanup_execution":{"$exists":false},
            "revision":signed(current.revision,"access grant revision").map_err(|_| AgentAccessError::Unavailable)?,
            "last_seq":signed(last_seq,"access grant event sequence").map_err(|_| AgentAccessError::Unavailable)?,
        }),crate::store::work_lease_update::replacement(sandbox_replacement_fields(&sandbox).map_err(|_| AgentAccessError::Unavailable)?))
            .session(&mut *session).await?;
        if result.matched_count != 1 {
            return Err(AgentAccessError::Conflict.into());
        }
        crate::store::sandbox_usage::save::record_saved(store, session, &sandbox, current.revision)
            .await
            .map_err(event_error)?;
        append_in_transaction(store, session, &current.id, at.clone(), event)
            .await
            .map_err(event_error)?;
    }
    let revision = record.revision;
    record.revision += 1;
    record.phase = AgentAccessPhase::Reviving;
    record.progress.grant_generation_floor =
        Some(sandbox.credential.service_access_generation_floor());
    record.updated_at = at;
    replace_leased(store, lease, revision, &record, session).await?;
    Ok(record)
}

fn event_error(error: TransactionEventError) -> Error {
    match error {
        TransactionEventError::Driver(error) => Error::Driver(error),
        _ => AgentAccessError::Unavailable.into(),
    }
}

#[cfg(test)]
#[path = "grant_tests.rs"]
pub(in crate::store) mod tests;
