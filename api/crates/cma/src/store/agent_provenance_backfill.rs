//! Throttled callers migrate one retained Agent at a time. No domain aggregate,
//! conversation, runtime attachment, ownership or event is rewritten.
use super::{
    agent_provenance::{document, read_snapshot},
    resource_access::{fence_lineage, ReadError},
    sandbox_mongo::{
        commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
        MongoSandboxStore, TRANSACTION_ATTEMPTS,
    },
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use layer2_core::{
    sandbox::store::{
        AgentProvenanceBackfillStore, Page, PageRequest, ProvenanceBackfillOutcome,
        ProvenanceBackfillProgress, SandboxStoreError,
    },
    sandbox::AgentCreationProvenance,
    AgentId,
};
use mongodb::{bson::doc, error::UNKNOWN_TRANSACTION_COMMIT_RESULT, ClientSession};

mod reconciliation;

#[async_trait::async_trait]
impl AgentProvenanceBackfillStore for MongoSandboxStore {
    async fn durable_provenance_backfill_step(
        &self,
        worker: &str,
    ) -> Result<Option<ProvenanceBackfillProgress>, SandboxStoreError> {
        self.provenance_progress_step(worker).await
    }

    async fn scan_agent_provenance_backfill(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentId>, SandboxStoreError> {
        PageRequest::new(page.limit, page.cursor.clone())?;
        let rows: Vec<_> = self
            .agents
            .find(doc! {"_id":{"$type":"string","$gt":page.cursor.as_deref().unwrap_or("")}})
            .projection(doc! {"_id":1})
            .sort(doc! {"_id":1})
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|_| unavailable())?
            .try_collect()
            .await
            .map_err(|_| unavailable())?;
        let mut ids: Vec<_> = rows
            .iter()
            .map(|row| {
                row.get_str("_id")
                    .map(|id| AgentId(id.into()))
                    .map_err(|_| {
                        SandboxStoreError::Corrupt("Agent backfill scan identity is invalid".into())
                    })
            })
            .collect::<Result<_, _>>()?;
        let next = (ids.len() > usize::from(page.limit))
            .then(|| ids[usize::from(page.limit) - 1].to_string());
        ids.truncate(usize::from(page.limit));
        Ok(Page { items: ids, next })
    }

    async fn backfill_agent_provenance(
        &self,
        id: &AgentId,
    ) -> Result<ProvenanceBackfillOutcome, SandboxStoreError> {
        let mut original = None;
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = match self.client.start_session().await {
                Ok(session) => session,
                Err(error) if is_transient(&error) => continue,
                Err(_) => return Err(unavailable()),
            };
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    continue;
                }
                return Err(unavailable());
            }
            let (outcome, receipt) =
                match apply_with_receipt(self, id, &mut session, &mut original).await {
                    Ok(outcome) => outcome,
                    Err(ReadError::Driver(error))
                        if !error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT)
                            && (is_transient(&error) || is_duplicate_key(&error)) =>
                    {
                        let _ = session.abort_guarded().await;
                        continue;
                    }
                    Err(error) => {
                        let _ = session.abort_guarded().await;
                        return Err(map_error(error));
                    }
                };
            if outcome == ProvenanceBackfillOutcome::Retained {
                let _ = session.abort_guarded().await;
                return Ok(outcome);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(outcome),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) => {
                    return reconciliation::after_unknown(self, receipt, session.operation_time())
                        .await;
                }
                CommitOutcome::Failed(_) => return Err(unavailable()),
            }
        }
        Err(unavailable())
    }
}

pub(super) async fn apply(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut ClientSession,
) -> Result<ProvenanceBackfillOutcome, ReadError> {
    apply_with_receipt(store, id, session, &mut None)
        .await
        .map(|(outcome, _)| outcome)
}

async fn apply_with_receipt(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut ClientSession,
    original: &mut Option<AgentCreationProvenance>,
) -> Result<(ProvenanceBackfillOutcome, AgentCreationProvenance), ReadError> {
    let snapshot = read_snapshot(store, id, session).await?;
    let provenance = snapshot
        .creation_provenance(id)
        .map_err(|_| layer2_core::resource_access::ResourceAccessStoreError::Corrupt)?;
    // A fresh snapshot may retry storage, never backfill a replacement Agent
    // under the original request merely because its identifier was reused.
    if original
        .as_ref()
        .is_some_and(|expected| expected != &provenance)
    {
        return Err(layer2_core::resource_access::ResourceAccessStoreError::Conflict.into());
    }
    original.get_or_insert_with(|| provenance.clone());
    if snapshot.creation.is_some() {
        return Ok((ProvenanceBackfillOutcome::Retained, provenance));
    }
    // Existing metadata-only lineage guards serialize with owner/share changes,
    // aggregate writes and purges. Serialized records and revisions are intact.
    fence_lineage(store, &snapshot.resource, session).await?;
    store
        .agent_creation_provenance
        .insert_one(
            document(&provenance)
                .map_err(|_| layer2_core::resource_access::ResourceAccessStoreError::Corrupt)?,
        )
        .session(session)
        .await?;
    Ok((ProvenanceBackfillOutcome::Recorded, provenance))
}

fn map_error(error: ReadError) -> SandboxStoreError {
    use layer2_core::resource_access::ResourceAccessStoreError;
    match error {
        ReadError::Domain(ResourceAccessStoreError::NotFound) => SandboxStoreError::NotFound,
        ReadError::Domain(ResourceAccessStoreError::Conflict) => SandboxStoreError::Conflict,
        ReadError::Domain(_) => {
            SandboxStoreError::Corrupt("Agent backfill evidence is invalid".into())
        }
        ReadError::Driver(_) => unavailable(),
    }
}
fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable("Agent provenance backfill is unavailable".into())
}
