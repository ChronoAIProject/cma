//! Mongo work discovery, lease claims, and terminal sandbox purge.

use futures_util::TryStreamExt as _;
use layer2_core::sandbox::store::{
    SandboxStore, SandboxStoreError, SandboxWorkerStore, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::SandboxStatus;
use layer2_core::SandboxId;
use mongodb::bson::{doc, Bson};

use super::sandbox_mongo::{
    commit_transaction, is_transient, signed, start_transaction, CommitOutcome, MongoSandboxStore,
    DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};
use super::sandboxes::read_sandbox;

#[async_trait::async_trait]
impl SandboxWorkerStore for MongoSandboxStore {
    async fn completed_installation_digest(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
    ) -> Result<Option<String>, SandboxStoreError> {
        super::native_prepared::completed_installation_digest(self, sandbox).await
    }

    async fn due_sandbox_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<SandboxId>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut cursor = self
            .sandboxes
            .find(doc! {
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "work_pending": true,
                "work_ready_at": { "$lte": now },
                "$or": [
                    { "work_lease_expires_at": Bson::Null },
                    { "work_lease_expires_at": { "$lte": now } },
                ],
            })
            .projection(doc! { "_id": 1 })
            .sort(doc! { "work_ready_at": 1, "_id": 1 })
            .limit(i64::from(limit))
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut ids = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            ids.push(SandboxId(
                document
                    .get_str("_id")
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
                    .to_string(),
            ));
        }
        Ok(ids)
    }

    async fn claim_sandbox_work(
        &self,
        id: &SandboxId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<layer2_core::sandbox::Sandbox>, SandboxStoreError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let Some(mut sandbox) = self.get_sandbox(id).await? else {
                return Ok(None);
            };
            if !sandbox.work.is_due(&now) || sandbox.lifecycle.checkpoint_capture.is_some() {
                return Ok(None);
            }
            let expected = sandbox.revision;
            sandbox
                .work
                .claim(holder.to_string(), &now, lease_expires_at.clone())
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            sandbox.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            sandbox.updated_at = now.clone();
            match self.save_sandbox(expected, sandbox.clone()).await {
                Ok(()) => return Ok(Some(sandbox)),
                Err(SandboxStoreError::Conflict) => tokio::task::yield_now().await,
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Conflict)
    }

    async fn purge_sandbox(
        &self,
        id: &SandboxId,
        expected_revision: u64,
        now: &str,
    ) -> Result<(), SandboxStoreError> {
        let sandbox = self
            .get_sandbox(id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if sandbox.revision != expected_revision
            || sandbox.status != SandboxStatus::Retired
            || !sandbox.credential.revoke_debt.is_empty()
            || sandbox
                .purge_after
                .as_deref()
                .is_none_or(|deadline| deadline > now)
        {
            return Err(SandboxStoreError::Conflict);
        }
        let mut last_transient = None;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::task::yield_now().await;
            }
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    continue;
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
            match purge_in_transaction(self, id, expected_revision, &mut session).await {
                Ok(()) => {}
                Err(PurgeError::Conflict) => {
                    let _ = session.abort_transaction().await;
                    return Err(SandboxStoreError::Conflict);
                }
                Err(PurgeError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(PurgeError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    return if self.get_sandbox(id).await?.is_none() {
                        Ok(())
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "sandbox purge commit remained unknown: {error}"
                        )))
                    };
                }
                CommitOutcome::Failed(error) => {
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
        }
        Err(SandboxStoreError::Unavailable(format!(
            "sandbox purge exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }
}

enum PurgeError {
    Conflict,
    Driver(mongodb::error::Error),
}

async fn purge_in_transaction(
    store: &MongoSandboxStore,
    id: &SandboxId,
    expected_revision: u64,
    session: &mut mongodb::ClientSession,
) -> Result<(), PurgeError> {
    let aggregate = store
        .sandboxes
        .find_one(doc! {
            "_id": id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(expected_revision, "sandbox purge revision")
                .map_err(|_| PurgeError::Conflict)?,
        })
        .session(&mut *session)
        .await
        .map_err(PurgeError::Driver)?;
    let Some(aggregate) = aggregate else {
        return Err(PurgeError::Conflict);
    };
    if super::ledger_collection_progress::retention_required(&aggregate)
        .map_err(|_| PurgeError::Conflict)?
    {
        return Err(PurgeError::Conflict);
    }
    if super::channel_storage_retention::required(
        &store.client.database(&store.sandboxes.namespace().db),
        id.as_str(),
        Some(&aggregate),
        Some(&mut *session),
    )
    .await
    .map_err(PurgeError::Driver)?
    {
        return Err(PurgeError::Conflict);
    }
    let sandbox = read_sandbox(&aggregate).map_err(|_| PurgeError::Conflict)?;
    if sandbox.status != SandboxStatus::Retired || !sandbox.credential.revoke_debt.is_empty() {
        return Err(PurgeError::Conflict);
    }
    if layer2_core::sandbox_usage::Allocation::from_sandbox(&sandbox)
        != layer2_core::sandbox_usage::Allocation::default()
    {
        return Err(PurgeError::Conflict);
    }
    let seq = aggregate
        .get_i64("last_seq")
        .map_err(|_| PurgeError::Conflict)?;
    let seq = u64::try_from(seq).map_err(|_| PurgeError::Conflict)?;
    super::sandbox_usage::record(
        store,
        session,
        &sandbox,
        seq,
        &layer2_core::now_rfc3339(),
        (seq, sandbox.revision),
        true,
    )
    .await
    .map_err(|error| match error {
        super::sandbox_events::TransactionEventError::Driver(error) => PurgeError::Driver(error),
        super::sandbox_events::TransactionEventError::Domain(_) => PurgeError::Conflict,
    })?;

    let mut cursor = store
        .agents
        .find(doc! { "sandbox_id": id.as_str() })
        .projection(doc! { "_id": 1 })
        .session(&mut *session)
        .await
        .map_err(PurgeError::Driver)?;
    let agent_documents = cursor
        .stream(&mut *session)
        .try_collect::<Vec<_>>()
        .await
        .map_err(PurgeError::Driver)?;
    let agent_ids = agent_documents
        .iter()
        .filter_map(|document| document.get_str("_id").ok())
        .collect::<Vec<_>>();
    if !agent_ids.is_empty() {
        store
            .agent_log
            .delete_many(doc! { "agent_id": { "$in": &agent_ids } })
            .session(&mut *session)
            .await
            .map_err(PurgeError::Driver)?;
        store
            .suggestions
            .delete_many(doc! { "agent_id": { "$in": &agent_ids } })
            .session(&mut *session)
            .await
            .map_err(PurgeError::Driver)?;
    }
    for collection in [
        &store.checkpoints,
        &store.sandbox_events,
        &store.agents,
        &store.agent_reuse_bindings,
        &store.worktrees,
        &store.workspaces,
    ] {
        collection
            .delete_many(doc! { "sandbox_id": id.as_str() })
            .session(&mut *session)
            .await
            .map_err(PurgeError::Driver)?;
    }
    let deleted = store
        .sandboxes
        .delete_one(doc! {
            "_id": id.as_str(),
            "revision": signed(expected_revision, "sandbox purge revision")
                .map_err(|_| PurgeError::Conflict)?,
        })
        .session(session)
        .await
        .map_err(PurgeError::Driver)?;
    if deleted.deleted_count == 1 {
        Ok(())
    } else {
        Err(PurgeError::Conflict)
    }
}
