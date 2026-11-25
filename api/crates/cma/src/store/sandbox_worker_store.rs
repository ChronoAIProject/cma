//! Mongo work discovery, lease claims, and terminal sandbox purge.

use crate::store::transaction_terminal::GuardedTerminal as _;
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
use super::work_claim_authority::without_ledger_maintenance;

#[async_trait::async_trait]
impl SandboxWorkerStore for MongoSandboxStore {
    async fn scope_sandbox_publication<'a>(
        &'a self,
        work: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<bool, SandboxStoreError>> + Send + 'a>,
        >,
    ) -> Result<bool, SandboxStoreError> {
        super::work_publication::scope(self, work).await
    }

    async fn completed_installation_digest(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
    ) -> Result<Option<String>, SandboxStoreError> {
        super::native_prepared::completed_installation_digest(self, sandbox).await
    }

    async fn canary_runtime_image(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
    ) -> Result<Option<layer3_adapter::RuntimeImage>, SandboxStoreError> {
        let database = self.client.database(&self.sandboxes.namespace().db);
        super::agent_lifecycle_operations::canary_admission::runtime_image(&database, sandbox).await
    }

    async fn admitted_runtime_release(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
    ) -> Result<Option<layer3_adapter::ReleasedRuntime>, SandboxStoreError> {
        let Some(lanes) = &self.released_lanes else {
            return Ok(None);
        };
        let database = self.client.database(&self.sandboxes.namespace().db);
        super::agent_lifecycle_operations::released::admitted_release(&database, lanes, sandbox)
            .await
    }

    async fn due_sandbox_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<SandboxId>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        if self
            .empty_discovery
            .empty(
                super::empty_discovery::Kind::Sandboxes,
                &self.sandboxes,
                doc! {"work_pending":true},
            )
            .await
        {
            return Ok(Vec::new());
        }
        let mut cursor = self
            .sandboxes
            .find(crate::store::work_schema::query(
                without_ledger_maintenance(doc! {
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "work_pending": true,
                    "$nor": [{"record.lifecycle.retirement_archive.verified": {"$type":"null"}}],
                    "work_ready_at": { "$lte": now },
                    "$or": [
                        { "work_lease_expires_at": Bson::Null },
                        { "work_lease_expires_at": { "$lte": now } },
                    ],
                }),
            ))
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
        self.empty_discovery
            .observed(super::empty_discovery::Kind::Sandboxes, ids.is_empty());
        Ok(ids)
    }

    async fn claim_sandbox_work(
        &self,
        id: &SandboxId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<layer2_core::sandbox::Sandbox>, SandboxStoreError> {
        let acquisition = super::work_claim_authority::eligible(&lease_expires_at)?;
        for _ in 0..TRANSACTION_ATTEMPTS {
            let Some(mut sandbox) = self.ordinary_work_sandbox(id).await? else {
                return Ok(None);
            };
            if !sandbox.work.is_due(&now)
                || sandbox.lifecycle.checkpoint_capture.is_some()
                || sandbox.retirement_archive_pending()
            {
                return Ok(None);
            }
            if self.renewable_work {
                sandbox.work.authority_protocol =
                    Some(layer2_core::sandbox::store::WorkAuthorityProtocol::RenewedV1);
            }
            let expected = sandbox.revision;
            sandbox
                .work
                .claim(holder.to_string(), &now, lease_expires_at.clone())
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            sandbox.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            sandbox.updated_at = now.clone();
            match self
                .save_sandbox_validated(expected, sandbox.clone(), Some(&acquisition))
                .await
            {
                Ok(()) => {
                    if sandbox.work.authority_protocol.is_none() {
                        return Ok(Some(sandbox));
                    }
                    let stored = self
                        .ordinary_work_sandbox(id)
                        .await?
                        .ok_or(SandboxStoreError::Conflict)?;
                    if stored
                        .work
                        .lease
                        .as_ref()
                        .map(|lease| (&lease.holder, lease.fence))
                        != sandbox
                            .work
                            .lease
                            .as_ref()
                            .map(|lease| (&lease.holder, lease.fence))
                        || stored.work.step != sandbox.work.step
                        || stored.owner != sandbox.owner
                    {
                        return Err(SandboxStoreError::Conflict);
                    }
                    return Ok(Some(stored));
                }
                Err(SandboxStoreError::Conflict) => tokio::task::yield_now().await,
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Conflict)
    }

    async fn renew_sandbox_work(
        &self,
        claimed: &layer2_core::sandbox::Sandbox,
        _now: &str,
        requested_until: &str,
    ) -> Result<Option<String>, SandboxStoreError> {
        super::work_lease_renewal::renew(
            &self.sandboxes,
            super::work_claim_authority::sandbox_claim(claimed)?,
            requested_until,
            None,
        )
        .await
    }

    async fn renew_sandbox_authority(
        &self,
        claimed: &layer2_core::sandbox::Sandbox,
        _now: &str,
        requested_until: &str,
    ) -> Result<Option<layer2_core::sandbox::store::WorkLeaseGrant>, SandboxStoreError> {
        super::work_lease_renewal::grant(
            &self.sandboxes,
            super::work_claim_authority::sandbox_claim(claimed)?,
            requested_until,
            None,
        )
        .await
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
                    let _ = session.abort_guarded().await;
                    return Err(SandboxStoreError::Conflict);
                }
                Err(PurgeError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(PurgeError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
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

impl MongoSandboxStore {
    async fn ordinary_work_sandbox(
        &self,
        id: &SandboxId,
    ) -> Result<Option<layer2_core::sandbox::Sandbox>, SandboxStoreError> {
        self.sandboxes
            .find_one(without_ledger_maintenance(doc! { "_id": id.as_str() }))
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_sandbox)
            .transpose()
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
    super::work_publication::fence_transaction(store, session)
        .await
        .map_err(|error| match error {
            super::work_publication::FenceError::Driver(error) => PurgeError::Driver(error),
            super::work_publication::FenceError::Domain(_) => PurgeError::Conflict,
        })?;
    let aggregate = store
        .sandboxes
        .find_one(crate::store::work_schema::query(doc! {
            "_id": id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(expected_revision, "sandbox purge revision")
                .map_err(|_| PurgeError::Conflict)?,
        }))
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
    if super::agent_log_segments::retention::required(store, id.as_str(), session)
        .await
        .map_err(PurgeError::Driver)?
    {
        return Err(PurgeError::Conflict);
    }
    let sandbox = read_sandbox(&aggregate).map_err(|_| PurgeError::Conflict)?;
    if store
        .agents
        .find_one(doc! {
            "sandbox_id": id.as_str(),
            "record.conversation.turn_timeout": {"$exists": true}
        })
        .session(&mut *session)
        .await
        .map_err(PurgeError::Driver)?
        .is_some()
    {
        return Err(PurgeError::Conflict);
    }
    if sandbox.retirement_archive_pending()
        || sandbox.status != SandboxStatus::Retired
        || sandbox.runtime.is_some()
        || !sandbox.credential.revoke_debt.is_empty()
    {
        return Err(PurgeError::Conflict);
    }
    if layer2_core::sandbox_usage::Allocation::from_sandbox(&sandbox)
        != layer2_core::sandbox_usage::Allocation::default()
    {
        return Err(PurgeError::Conflict);
    }
    if super::sandbox_credentials::purge::required(
        &store.client.database(&store.sandboxes.namespace().db),
        &aggregate,
        &sandbox,
        session,
    )
    .await
    .map_err(PurgeError::Driver)?
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
        .projection(doc! { "_id": 1, "title_effect_floor": 1, "title_effect_custody": 1 })
        .session(&mut *session)
        .await
        .map_err(PurgeError::Driver)?;
    let agent_documents = cursor
        .stream(&mut *session)
        .try_collect::<Vec<_>>()
        .await
        .map_err(PurgeError::Driver)?;
    super::agent_title_effect::purge::require_finality(store, &agent_documents, session)
        .await
        .map_err(|error| match error {
            super::agent_log_integrity::TransactionLogError::Driver(error) => {
                PurgeError::Driver(error)
            }
            super::agent_log_integrity::TransactionLogError::Domain(_) => PurgeError::Conflict,
        })?;
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
        .delete_one(crate::store::work_schema::query(
            super::work_publication::sandbox(
                doc! {
                    "_id": id.as_str(),
                    "revision": signed(expected_revision, "sandbox purge revision")
                        .map_err(|_| PurgeError::Conflict)?,
                },
                id.as_str(),
            )
            .map_err(|_| PurgeError::Conflict)?,
        ))
        .session(session)
        .await
        .map_err(PurgeError::Driver)?;
    if deleted.deleted_count == 1 {
        Ok(())
    } else {
        Err(PurgeError::Conflict)
    }
}
