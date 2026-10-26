//! Commit exact owner scope and same-runtime credential renewal with its lease.
use super::*;
use crate::store::sandbox_events::{
    append_in_transaction, validate_in_transaction, TransactionEventError,
};
use crate::store::sandbox_mongo::{signed, unsigned};
use crate::store::sandboxes::{read_sandbox, sandbox_replacement_fields};
use layer2_core::sandbox::{Sandbox, SandboxEventKind};

impl MongoSandboxStore {
    pub(crate) async fn grant_live_access_request_services(
        &self,
        lease: &AgentAccessLease,
    ) -> Result<AgentAccessRecord, AgentAccessError> {
        let record = transaction!(self, session, { apply(self, lease, session).await })?;
        // Only a wake hint: the ordinary worker still claims the durable work.
        self.sandbox_admitted.notify_one();
        Ok(record)
    }
}

async fn apply(
    store: &MongoSandboxStore,
    lease: &AgentAccessLease,
    session: &mut ClientSession,
) -> TxResult<AgentAccessRecord> {
    let mut record = super::worker::leased(store, lease, session).await?;
    if !matches!(
        record.phase,
        AgentAccessPhase::Connecting | AgentAccessPhase::WaitingQuiet | AgentAccessPhase::Updating
    ) {
        return Err(AgentAccessError::Conflict.into());
    }
    let snapshot = validate_lineage(store, &record.requester, session, false).await?;
    let value = store
        .sandboxes
        .find_one(doc! {"_id": record.requester.sandbox_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(AgentAccessError::NotFound)?;
    let current = read_sandbox(&value).map_err(|_| AgentAccessError::Unavailable)?;
    if record.phase == AgentAccessPhase::Updating {
        validate_replay(&record, &current)?;
        fence(store, &snapshot, session).await?;
        return Ok(record);
    }
    let services = record.approved_services()?.to_vec();
    let expected_revision = record.expected_scope_revision.unwrap_or(0);
    let effective = current.credential.effective_reaches();
    if record.progress != AgentAccessProgress::default()
        || expected_revision
            != current
                .credential
                .service_scope
                .as_ref()
                .map_or(0, |scope| scope.revision)
        || match &record.scope_mode {
            AgentAccessScopeMode::Additive => {
                !effective.iter().all(|service| services.contains(service))
            }
            AgentAccessScopeMode::Replace { expected_services } => expected_services != &effective,
        }
    {
        return Err(AgentAccessError::Invalid.into());
    }
    if store
        .agent_access_requests
        .count_documents(doc! {
            "_id":{"$ne":&record.id}, "record.requester.sandbox_id":current.id.as_str(),
            "record.phase":{"$in":APPLYING},
        })
        .session(&mut *session)
        .await?
        > 0
    {
        return Err(AgentAccessError::Conflict.into());
    }
    let runtime = current.runtime.clone().ok_or(AgentAccessError::Conflict)?;
    let agent = snapshot
        .resource
        .lineage
        .agent
        .as_ref()
        .ok_or(AgentAccessError::NotFound)?;
    if runtime.provision_generation.is_none()
        || agent.conversation.runtime_generation != runtime.provision_generation
    {
        return Err(AgentAccessError::Conflict.into());
    }
    let last_seq = unsigned(
        value
            .get_i64("last_seq")
            .map_err(|_| AgentAccessError::Unavailable)?,
        "live access event sequence",
    )
    .map_err(|_| AgentAccessError::Unavailable)?;
    validate_in_transaction(store, &current.id, last_seq, session)
        .await
        .map_err(event_error)?;
    let at = layer2_core::now_rfc3339();
    let mut sandbox = current.clone();
    let scope = sandbox
        .revise_live_service_access(&record.requester.owner, &services, at.clone())
        .map_err(|_| AgentAccessError::Conflict)?;
    let floor = scope.credential_generation_floor;
    let event = SandboxEventKind::ServiceAccessRevised {
        scope: Box::new(scope),
    };
    current
        .validate_service_access_transaction(&sandbox, &event, &at)
        .map_err(|_| AgentAccessError::Conflict)?;
    fence(store, &snapshot, session).await?;
    let result = store.sandboxes.update_one(doc! {
        "_id":current.id.as_str(), "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "channel_reuse_cleanup_execution":{"$exists":false},
        "revision":signed(current.revision,"live access revision").map_err(|_| AgentAccessError::Unavailable)?,
        "last_seq":signed(last_seq,"live access event sequence").map_err(|_| AgentAccessError::Unavailable)?,
    }, doc! {"$set":sandbox_replacement_fields(&sandbox).map_err(|_| AgentAccessError::Unavailable)?})
        .session(&mut *session).await?;
    if result.matched_count != 1 {
        return Err(AgentAccessError::Conflict.into());
    }
    append_in_transaction(store, session, &current.id, at.clone(), event)
        .await
        .map_err(event_error)?;
    let revision = record.revision;
    record.revision = record
        .revision
        .checked_add(1)
        .ok_or(AgentAccessError::Unavailable)?;
    record.phase = AgentAccessPhase::Updating;
    record.progress.hot_update = Some(AgentAccessHotUpdate {
        runtime,
        lifecycle_next_authority: current.lifecycle.next_authority,
    });
    record.progress.grant_generation_floor = Some(floor);
    record.updated_at = at;
    replace_leased(store, lease, revision, &record, session).await?;
    Ok(record)
}

fn validate_replay(record: &AgentAccessRecord, sandbox: &Sandbox) -> Result<(), AgentAccessError> {
    let binding = record
        .progress
        .hot_update
        .as_ref()
        .ok_or(AgentAccessError::Conflict)?;
    let scope = sandbox
        .credential
        .service_scope
        .as_ref()
        .ok_or(AgentAccessError::Conflict)?;
    if sandbox.runtime.as_ref() != Some(&binding.runtime)
        || sandbox.lifecycle.next_authority != binding.lifecycle_next_authority
        || scope.revision
            != record
                .expected_scope_revision
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(AgentAccessError::Invalid)?
        || scope.authorized_by != record.requester.owner
        || scope.services != record.approved_services()?
        || Some(scope.credential_generation_floor) != record.progress.grant_generation_floor
    {
        return Err(AgentAccessError::Conflict);
    }
    Ok(())
}

fn event_error(error: TransactionEventError) -> Error {
    match error {
        TransactionEventError::Driver(error) => Error::Driver(error),
        _ => AgentAccessError::Unavailable.into(),
    }
}

#[cfg(test)]
#[path = "hot_grant_tests.rs"]
mod tests;
