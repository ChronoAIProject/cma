//! Bounded Agent-side channel history. Callers must separately fence bridge
//! authority; this adapter never grants a binding access to the Agent.
use super::{
    resource_access::{fence_lineage, ReadError},
    sandbox_mongo::{
        commit_transaction, is_confirmed_body_retry, is_transient, start_transaction,
        CommitOutcome, MongoSandboxStore, TRANSACTION_ATTEMPTS,
    },
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::{
    sandbox::{
        channel_usage::*,
        store::{
            AgentChannelUsageSnapshot, AgentChannelUsageStore, Page, PageRequest, SandboxStoreError,
        },
    },
    AgentId,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

mod reconciliation;
mod records;
pub(super) use records::ensure_indexes;
pub(super) use records::read_snapshot as read_inventory_snapshot;
use records::{event_document, read_snapshot, state_document};
pub(super) use records::{read_event, read_state};

#[derive(Debug)]
pub(super) enum UsageError {
    Driver(mongodb::error::Error),
    Domain(SandboxStoreError),
}
impl From<mongodb::error::Error> for UsageError {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
impl From<SandboxStoreError> for UsageError {
    fn from(value: SandboxStoreError) -> Self {
        Self::Domain(value)
    }
}
impl From<ReadError> for UsageError {
    fn from(value: ReadError) -> Self {
        use layer2_core::resource_access::ResourceAccessStoreError;
        match value {
            ReadError::Driver(error) => Self::Driver(error),
            ReadError::Domain(ResourceAccessStoreError::NotFound) => {
                Self::Domain(SandboxStoreError::NotFound)
            }
            ReadError::Domain(ResourceAccessStoreError::Conflict) => {
                Self::Domain(SandboxStoreError::Conflict)
            }
            ReadError::Domain(_) => Self::Domain(SandboxStoreError::Corrupt(
                "channel usage lineage is invalid".into(),
            )),
        }
    }
}
fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable("channel usage store is unavailable".into())
}
fn map_error(error: UsageError) -> SandboxStoreError {
    match error {
        UsageError::Domain(error) => error,
        UsageError::Driver(_) => unavailable(),
    }
}

#[async_trait::async_trait]
impl AgentChannelUsageStore for MongoSandboxStore {
    async fn agent_channel_usage_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<AgentChannelUsageSnapshot, SandboxStoreError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = match begin(self).await {
                Ok(session) => session,
                // A known-aborted start (including the work fence) retries
                // from a fresh transaction under the same bounded budget.
                Err(error) if known_abort(&error) => continue,
                Err(_) => return Err(unavailable()),
            };
            let result = read_snapshot(self, id, &mut session).await;
            let _ = session.abort_guarded().await;
            match result {
                Ok(snapshot) => return Ok(snapshot),
                Err(UsageError::Driver(error)) if is_transient(&error) => continue,
                Err(error) => return Err(map_error(error)),
            }
        }
        Err(unavailable())
    }

    async fn record_agent_channel_usage(
        &self,
        request: ChannelUsageMutation,
    ) -> Result<ChannelUsageEvent, SandboxStoreError> {
        request.validate()?;
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = match begin(self).await {
                Ok(session) => session,
                // A known-aborted start (including the work fence) retries
                // from a fresh transaction under the same bounded budget.
                Err(error) if known_abort(&error) => continue,
                Err(_) => return Err(unavailable()),
            };
            let (event, replay) = match apply(self, &request, &mut session).await {
                Ok(result) => result,
                Err(UsageError::Driver(error)) if is_confirmed_body_retry(&error) => {
                    let _ = session.abort_guarded().await;
                    continue;
                }
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    return Err(map_error(error));
                }
            };
            if replay {
                let _ = session.abort_guarded().await;
                return Ok(event);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(event),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) => {
                    return reconciliation::verify(self, &request, event, session.operation_time())
                        .await;
                }
                CommitOutcome::Failed(_) => return Err(unavailable()),
            }
        }
        Err(unavailable())
    }

    async fn agent_channel_usage_history(
        &self,
        id: &AgentId,
        page: &PageRequest,
    ) -> Result<(AgentChannelUsageSnapshot, Page<ChannelUsageEvent>), SandboxStoreError> {
        layer2_core::sandbox::store::channel_usage_cursor(page)?;
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = match begin(self).await {
                Ok(session) => session,
                // A known-aborted start (including the work fence) retries
                // from a fresh transaction under the same bounded budget.
                Err(error) if known_abort(&error) => continue,
                Err(_) => return Err(unavailable()),
            };
            let result = records::read_history(self, id, page, &mut session).await;
            let _ = session.abort_guarded().await;
            match result {
                Ok(result) => return Ok(result),
                Err(UsageError::Driver(error)) if is_transient(&error) => continue,
                Err(error) => return Err(map_error(error)),
            }
        }
        Err(unavailable())
    }
}

/// Unknown outcome dominates a transient label: only a known abort retries.
fn known_abort(error: &mongodb::error::Error) -> bool {
    !error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT) && is_transient(error)
}

async fn begin(store: &MongoSandboxStore) -> Result<ClientSession, mongodb::error::Error> {
    let mut session = store.client.start_session().await?;
    start_transaction(&mut session).await?;
    Ok(session)
}

/// Composes with authorized Bridge transactions so binding and accepted-command
/// history commits atomically with its actual source action.
pub(super) async fn apply(
    store: &MongoSandboxStore,
    request: &ChannelUsageMutation,
    session: &mut ClientSession,
) -> Result<(ChannelUsageEvent, bool), UsageError> {
    let id = &request.creation.agent_id;
    let current = read_snapshot(store, id, session).await?;
    request.require_current(&current.provenance)?;
    if let Some(row) = store
        .channel_usage_events
        .find_one(doc! {"_id":request.operation_id()})
        .session(&mut *session)
        .await?
    {
        return Ok((request.replay(read_event(&row)?)?, true));
    }
    let binding = request.change.introduced_binding();
    if let Some(binding) = binding {
        if store
            .channel_usage_events
            .find_one(doc! {"binding_id":&binding.binding_id})
            .session(&mut *session)
            .await?
            .is_some()
        {
            return Err(SandboxStoreError::Conflict.into());
        }
    }
    let predecessor = match binding.and_then(|binding| binding.previous_binding_id.as_ref()) {
        Some(previous) => {
            let row = store
                .channel_usage_events
                .find_one(doc! {"$or":[
                    {"binding_id":previous},
                    {"record.predecessor.binding_id":previous,"record.change.kind":"use"}
                ]})
                .session(&mut *session)
                .await?
                .ok_or(SandboxStoreError::Conflict)?;
            let event = read_event(&row)?;
            Some(
                event
                    .change
                    .introduced_binding()
                    .or_else(|| {
                        matches!(event.change, ChannelUsageChange::Use { .. })
                            .then_some(event.predecessor.as_ref())
                            .flatten()
                    })
                    .filter(|binding| &binding.binding_id == previous)
                    .ok_or_else(|| {
                        SandboxStoreError::Corrupt("channel binding index changed".into())
                    })?
                    .clone(),
            )
        }
        None => None,
    };
    let event = plan_channel_usage(request, &current.provenance, &current.state, predecessor)?;
    fence_lineage(store, &current.provenance.resource, session).await?;
    store
        .channel_usage_events
        .insert_one(event_document(&event)?)
        .session(&mut *session)
        .await?;
    let next = StoredAgentChannelUsage {
        state: event.after.clone(),
        latest_event_digest: event.digest(),
    };
    let document = state_document(&next)?;
    if current.state.revision == 0 {
        store
            .agent_channel_usage
            .insert_one(document)
            .session(session)
            .await?;
    } else {
        let result = store
            .agent_channel_usage
            .replace_one(
                doc! {"_id":id.as_str(),"revision":current.state.revision as i64},
                document,
            )
            .session(session)
            .await?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict.into());
        }
    }
    Ok((event, false))
}
