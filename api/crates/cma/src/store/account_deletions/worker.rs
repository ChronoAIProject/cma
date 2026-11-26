pub(super) use super::worker_records::{Accepted, CheckpointRow, Kind, OperationRow};
pub(super) use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
pub(super) use layer2_core::account_deletion::worker::*;
pub(super) use mongodb::ClientSession;

pub(super) type Attempt<T> = Result<T, AttemptError>;
pub(super) enum AttemptError {
    Domain(JournalError),
    Driver(mongodb::error::Error),
}
impl From<JournalError> for AttemptError {
    fn from(value: JournalError) -> Self {
        Self::Domain(value)
    }
}
impl From<mongodb::error::Error> for AttemptError {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
pub(super) static COMMITS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(16);

#[derive(Clone)]
pub(super) struct MongoErasureJournal {
    pub(super) database: Database,
    pub(super) checkpoints: Collection<Document>,
    pub(super) operations: Collection<Document>,
    #[cfg(test)]
    pub(super) pause: Option<std::sync::Arc<super::commit_pause::Pause>>,
}
impl MongoErasureJournal {
    pub(super) fn from_database(database: &Database) -> Self {
        Self {
            database: database.clone(),
            checkpoints: crate::store::durability::majority_collection(
                database,
                worker_schema::WORK,
            ),
            operations: crate::store::durability::majority_collection(
                database,
                worker_schema::EFFECTS,
            ),
            #[cfg(test)]
            pause: None,
        }
    }
    pub(super) fn parents(&self) -> Collection<Document> {
        crate::store::durability::majority_collection(&self.database, RECORDS)
    }
    pub(super) async fn enrolled(
        &self,
        id: &AccountDeletionId,
        session: &mut ClientSession,
    ) -> Result<bool, mongodb::error::Error> {
        // The caller has already read the parent in this transaction. If schema
        // preparation/enrollment races this metadata read, their parent write
        // conflicts with the caller's final write. Never cache this absence.
        if self
            .database
            .list_collection_names()
            .filter(doc! {"name":worker_schema::WORK})
            .await?
            .is_empty()
        {
            return Ok(false);
        }
        self.checkpoints
            .find_one(doc! {"_id":id.as_str()})
            .projection(doc! {"_id":1})
            .session(session)
            .await
            .map(|v| v.is_some())
    }
    #[cfg(test)]
    pub(super) async fn pause_at(&self, point: super::commit_pause::Point) {
        if let Some(pause) = &self.pause {
            pause.hit(point).await;
        }
    }
    async fn apply(
        &self,
        request: &JournalRequest,
        claim: Option<&JournalClaim>,
        budget: &JournalBudget,
    ) -> Result<JournalOutcome, JournalError> {
        request.validate()?;
        let permit = tokio::time::timeout(budget.remaining()?, COMMITS.acquire())
            .await
            .map_err(|_| JournalError::Cancelled)?
            .map_err(|_| JournalError::Unavailable)?;
        tokio::time::timeout(budget.remaining()?, worker_schema::verify(&self.database))
            .await
            .map_err(|_| JournalError::Cancelled)??;
        let token = uuid::Uuid::new_v4().to_string();
        for _ in 0..8 {
            let mut session = tokio::time::timeout(
                budget.remaining()?,
                self.database
                    .client()
                    .start_session()
                    .default_transaction_options(
                        mongodb::options::TransactionOptions::builder()
                            .max_commit_time(Duration::from_secs(5))
                            .build(),
                    ),
            )
            .await
            .map_err(|_| JournalError::Cancelled)?
            .map_err(|_| JournalError::Unavailable)?;
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    continue;
                }
                return Err(JournalError::Unavailable);
            }
            let staged = tokio::time::timeout(
                budget.remaining()?,
                super::worker_journal::stage(self, request, claim, &token, &mut session),
            )
            .await;
            let (outcome, wrote) = match staged {
                Ok(Ok(value)) => value,
                failure => {
                    let _ = session.abort_guarded().await;
                    match failure {
                        Err(_) => return Err(JournalError::Cancelled),
                        Ok(Err(AttemptError::Domain(error))) => return Err(error),
                        Ok(Err(AttemptError::Driver(error))) if is_transient(&error) => continue,
                        _ => return Err(JournalError::Unavailable),
                    }
                }
            };
            if !wrote {
                let _ = session.abort_guarded().await;
                return Ok(outcome);
            }
            #[cfg(test)]
            self.pause_at(super::commit_pause::Point::AfterGuard).await;
            let remaining = match budget.remaining() {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
            };
            // The task retains session ownership after submission, even if the caller
            // stops waiting. A timeout can only return the original recovery identity.
            let submitted = tokio::spawn(async move {
                let result = commit_transaction(&mut session).await;
                drop(permit);
                result
            });
            let result = tokio::time::timeout(remaining, submitted).await;
            #[cfg(test)]
            self.pause_at(super::commit_pause::Point::AfterCommit).await;
            return match result {
                Ok(Ok(CommitOutcome::Committed)) if budget.remaining().is_ok() => Ok(outcome),
                Ok(Ok(CommitOutcome::Retry(_) | CommitOutcome::Failed(_)))
                    if budget.remaining().is_ok() =>
                {
                    Err(JournalError::Conflict)
                }
                _ => Ok(JournalOutcome::RecoveryPending {
                    operation: request.key().into(),
                }),
            };
        }
        Err(JournalError::Conflict)
    }
}

#[async_trait::async_trait]
impl ErasureJournal for MongoErasureJournal {
    async fn inspect(
        &self,
        id: &AccountDeletionId,
    ) -> Result<Option<JournalCheckpoint>, JournalError> {
        worker_schema::verify(&self.database).await?;
        self.checkpoints
            .find_one(doc! {"_id":id.as_str()})
            .max_time(Duration::from_secs(5))
            .await
            .map_err(|_| JournalError::Unavailable)?
            .map(super::worker_records::checkpoint)
            .transpose()
            .map(|row| row.map(|row| row.checkpoint))
    }
    async fn enroll(
        &self,
        request: &JournalRequest,
        budget: &JournalBudget,
    ) -> Result<JournalOutcome, JournalError> {
        if !matches!(request.command(), JournalCommand::Enroll) {
            return Err(JournalError::Invalid);
        }
        self.apply(request, None, budget).await
    }
    async fn claim_due(
        &self,
        request: &JournalRequest,
        budget: &JournalBudget,
    ) -> Result<JournalOutcome, JournalError> {
        if !matches!(request.command(), JournalCommand::Claim { .. }) {
            return Err(JournalError::Invalid);
        }
        self.apply(request, None, budget).await
    }
    async fn commit(
        &self,
        claim: &JournalClaim,
        request: &JournalRequest,
        budget: &JournalBudget,
    ) -> Result<JournalOutcome, JournalError> {
        if matches!(
            request.command(),
            JournalCommand::Claim { .. } | JournalCommand::Enroll
        ) {
            return Err(JournalError::Invalid);
        }
        self.apply(request, Some(claim), budget).await
    }
    async fn resolve(
        &self,
        request: &JournalRequest,
        fence: Option<&JournalClaim>,
        budget: &JournalBudget,
    ) -> Result<JournalResolution, JournalError> {
        super::worker_recovery::resolve(self, request, fence, budget).await
    }
}
