//! Transactional judgement and exactly-once reservation release.
use super::quota::{
    self, number, object_identity_matches, object_row, owner_id, unavailable, validate_row, IO,
    QUOTA_COLLECTION, STATE,
};
use crate::store::{commit_transaction, is_transient, start_transaction, CommitOutcome};
use layer2_core::document_artifacts::*;
use mongodb::{
    bson::{doc, Document, Timestamp},
    options::{ReadConcern, WriteConcern},
    ClientSession, Collection, Database,
};

mod delete;
mod queries;
mod transactions;

pub(crate) struct MongoDocumentReclaimStore {
    pub(super) rows: Collection<Document>,
    pub(super) documents: Collection<Document>,
    pub(super) lifecycle: Collection<Document>,
    enabled: bool,
}

impl MongoDocumentReclaimStore {
    pub(crate) fn new(database: &Database, quotas_enabled: bool) -> Self {
        let options = mongodb::options::CollectionOptions::builder()
            .read_concern(ReadConcern::majority())
            .write_concern(WriteConcern::majority())
            .build();
        Self {
            rows: database.collection_with_options(QUOTA_COLLECTION, options.clone()),
            documents: database.collection_with_options(super::COLLECTION, options.clone()),
            lifecycle: database.collection_with_options(super::LIFECYCLE_COLLECTION, options),
            enabled: quotas_enabled,
        }
    }

    pub(crate) fn purge(&self) -> super::purge::MongoDocumentPurgeStore {
        super::purge::MongoDocumentPurgeStore::new(
            &self.rows.client().database(&self.rows.namespace().db),
        )
    }
    async fn fence_lifecycle(
        &self,
        candidate: &ObjectCandidate,
        session: &mut ClientSession,
    ) -> Result<bool, TransactionError> {
        // Serialize judgement/claim/finalization with reserve and logical deletion.
        self.rows
            .update_one(
                doc! {"_id":owner_id(&candidate.source.owner_hash)},
                doc! {"$inc":{"fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        let row = self
            .lifecycle
            .find_one(doc! {"_id":&candidate.source.document_id})
            .session(session)
            .await?;
        Ok(row
            .as_ref()
            .is_some_and(|r| matches!(r.get_str("state").ok(), Some("deleting" | "purged"))))
    }
    async fn run(
        &self,
        candidate: &ObjectCandidate,
        action: Action<'_>,
    ) -> Result<Applied, DocumentError> {
        validate_candidate(candidate)?;
        tokio::time::timeout(IO, async {
            for attempt in 0..8 {
                let mut session = self
                    .rows
                    .client()
                    .start_session()
                    .await
                    .map_err(unavailable)?;
                if let Err(error) = start_transaction(&mut session).await {
                    if is_transient(&error) {
                        continue;
                    }
                    return Err(DocumentError::Unavailable);
                }
                let result = match action {
                    Action::Judge => self
                        .judge_transaction(candidate, &mut session)
                        .await
                        .map(Applied::Judged),
                    Action::Finalize(claim) => self
                        .finalize_transaction(claim, &mut session)
                        .await
                        .map(Applied::Finalized),
                };
                let applied = match result {
                    Ok(applied) => applied,
                    Err(TransactionError::Domain(error)) => {
                        let _ = session.abort_transaction().await;
                        return Err(error);
                    }
                    Err(TransactionError::Driver(error)) if is_transient(&error) => {
                        let _ = session.abort_transaction().await;
                        tokio::time::sleep(std::time::Duration::from_millis(attempt + 1)).await;
                        continue;
                    }
                    Err(_) => {
                        let _ = session.abort_transaction().await;
                        return Err(DocumentError::Unavailable);
                    }
                };
                match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(applied),
                    CommitOutcome::Retry(_) => {
                        let _ = session.abort_transaction().await;
                    }
                    CommitOutcome::Unknown(_) => {
                        return self
                            .reconcile(candidate, applied, session.operation_time())
                            .await;
                    }
                    CommitOutcome::Failed(_) => return Err(DocumentError::Unavailable),
                }
            }
            Err(DocumentError::Unavailable)
        })
        .await
        .map_err(|_| DocumentError::Unavailable)?
    }

    async fn reconcile(
        &self,
        candidate: &ObjectCandidate,
        applied: Applied,
        time: Option<Timestamp>,
    ) -> Result<Applied, DocumentError> {
        let Some(time) = time else {
            return Ok(applied.not_processed());
        };
        let mut snapshot = self
            .rows
            .client()
            .start_session()
            .snapshot(true)
            .snapshot_time(time)
            .await
            .map_err(unavailable)?;
        let row = self
            .rows
            .find_one(doc! {"_id":id(candidate)?})
            .read_concern(ReadConcern::snapshot())
            .max_time(IO)
            .session(&mut snapshot)
            .await
            .map_err(unavailable)?;
        Ok(match (&applied, row.as_ref()) {
            (Applied::Judged(Judgement::Committed), Some(row))
                if same_incarnation(row, candidate)
                    && row.get_str("state").ok() == Some("committed") =>
            {
                applied
            }
            (Applied::Judged(Judgement::Condemned), Some(row))
                if same_incarnation(row, candidate)
                    && row.get_str("state").ok() == Some("condemned") =>
            {
                applied
            }
            // An absent row proves finality, but cannot identify which process
            // released it. Conservatively avoid attributing a second release.
            _ => applied.not_processed(),
        })
    }

    async fn activation_time(&self) -> Result<Option<mongodb::bson::DateTime>, DocumentError> {
        let state = self
            .rows
            .find_one(doc! {"_id":STATE})
            .max_time(IO)
            .await
            .map_err(unavailable)?;
        Ok(state.and_then(|row| row.get_datetime("activated_at").ok().copied()))
    }
}

#[async_trait::async_trait]
impl DocumentReclaimStore for MongoDocumentReclaimStore {
    async fn ready(&self) -> Result<bool, DocumentError> {
        if !self.enabled {
            return Ok(false);
        }
        Ok(self
            .rows
            .find_one(doc! {"_id":STATE})
            .max_time(IO)
            .await
            .map_err(unavailable)?
            .is_some_and(|state| quota::ready(&state)))
    }
    async fn candidates(
        &self,
        reserved_before: i64,
        limit: usize,
    ) -> Result<Vec<ObjectCandidate>, DocumentError> {
        self.find_candidates(reserved_before, limit.min(RECLAIM_BATCH))
            .await
    }
    async fn judge(&self, candidate: &ObjectCandidate) -> Result<Judgement, DocumentError> {
        match self.run(candidate, Action::Judge).await? {
            Applied::Judged(result) => Ok(result),
            _ => Err(DocumentError::Corrupt),
        }
    }
    async fn claim_delete(
        &self,
        candidate: &ObjectCandidate,
        holder: &str,
        now: i64,
    ) -> Result<Option<DeleteClaim>, DocumentError> {
        self.claim_deletion(candidate, holder, now).await
    }
    async fn finalize(&self, claim: &DeleteClaim) -> Result<bool, DocumentError> {
        delete::claim_filter(claim)?;
        match self.run(&claim.candidate, Action::Finalize(claim)).await? {
            Applied::Finalized(result) => Ok(result),
            _ => Err(DocumentError::Corrupt),
        }
    }
    async fn backlog(&self, now: i64) -> Result<ReclaimBacklog, DocumentError> {
        self.find_backlog(now).await
    }
}

#[derive(Clone, Copy)]
enum Action<'a> {
    Judge,
    Finalize(&'a DeleteClaim),
}
enum Applied {
    Judged(Judgement),
    Finalized(bool),
}
impl Applied {
    fn not_processed(self) -> Self {
        match self {
            Self::Judged(_) => Self::Judged(Judgement::Skipped),
            Self::Finalized(_) => Self::Finalized(false),
        }
    }
}
fn id(candidate: &ObjectCandidate) -> Result<String, DocumentError> {
    Ok(object_row(&candidate.source)?
        .get_str("_id")
        .map_err(|_| DocumentError::Corrupt)?
        .into())
}
fn validate_candidate(candidate: &ObjectCandidate) -> Result<(), DocumentError> {
    candidate.source.validate()?;
    if candidate
        .reservation_id
        .as_deref()
        .is_some_and(|id| !super::quota_baseline::valid_activation(id))
    {
        return Err(DocumentError::Corrupt);
    }
    Ok(())
}
fn incarnation_filter(candidate: &ObjectCandidate) -> Document {
    match &candidate.reservation_id {
        Some(value) => doc! {"reservation_id": value},
        None => doc! {"reservation_id": {"$exists": false}},
    }
}
fn same_incarnation(row: &Document, candidate: &ObjectCandidate) -> bool {
    match (&candidate.reservation_id, row.get("reservation_id")) {
        (Some(expected), Some(mongodb::bson::Bson::String(actual))) => expected == actual,
        (None, None) => true,
        _ => false,
    }
}
enum TransactionError {
    Domain(DocumentError),
    Driver(mongodb::error::Error),
}
impl From<DocumentError> for TransactionError {
    fn from(error: DocumentError) -> Self {
        Self::Domain(error)
    }
}
impl From<mongodb::error::Error> for TransactionError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}
