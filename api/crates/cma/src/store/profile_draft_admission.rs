//! Read erasure fences before committing newly usable draft/default/fork pins.
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::{
    bson::{doc, Document},
    error::{ErrorKind, WriteFailure, UNKNOWN_TRANSACTION_COMMIT_RESULT},
    Client, Collection, Database,
};

use super::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};

const WRITE_CONFLICT: i32 = 112;

mod pins;
pub(super) use pins::{Pin, Pins};
mod profiles;
pub(super) use profiles::{create, save};

pub(super) enum Error {
    ReferenceUnavailable,
    Driver(mongodb::error::Error),
}

pub(super) enum Mutation {
    Insert {
        collection: Collection<Document>,
        document: Document,
    },
    Update {
        collection: Collection<Document>,
        filter: Document,
        update: Document,
    },
    Replace {
        collection: Collection<Document>,
        filter: Document,
        document: Document,
    },
    Settings {
        collection: Collection<Document>,
        filter: Document,
        pipeline: Vec<Document>,
        upsert: bool,
    },
}

impl Mutation {
    fn collection(&self) -> &Collection<Document> {
        match self {
            Self::Insert { collection, .. }
            | Self::Update { collection, .. }
            | Self::Replace { collection, .. }
            | Self::Settings { collection, .. } => collection,
        }
    }

    async fn retire_failed_commit(&self, session: &mut mongodb::ClientSession) {
        // The driver marks the transaction committed before sending commit,
        // so its abort API cannot retire a failed commit's open server state.
        // A read-only successor advances txnNumber without undoing any commit.
        if start_transaction(session).await.is_ok() {
            let _ = self
                .collection()
                .find_one(doc! {"_id":{"$in":[]}})
                .projection(doc! {"_id":1_i32})
                .session(&mut *session)
                .await;
            let _ = session.abort_guarded().await;
        }
    }

    async fn apply(
        &self,
        session: &mut mongodb::ClientSession,
    ) -> mongodb::error::Result<Option<Document>> {
        match self {
            Self::Insert {
                collection,
                document,
            } => {
                collection
                    .insert_one(document.clone())
                    .session(session)
                    .await?;
                Ok(Some(document.clone()))
            }
            Self::Update {
                collection,
                filter,
                update,
            } => {
                let result = collection
                    .update_one(filter.clone(), update.clone())
                    .session(session)
                    .await?;
                Ok((result.matched_count == 1).then_some(doc! {}))
            }
            Self::Replace {
                collection,
                filter,
                document,
            } => {
                let result = collection
                    .replace_one(filter.clone(), document.clone())
                    .session(session)
                    .await?;
                Ok((result.matched_count == 1).then(|| document.clone()))
            }
            Self::Settings {
                collection,
                filter,
                pipeline,
                upsert,
            } => {
                collection
                    .find_one_and_update(filter.clone(), pipeline.clone())
                    .return_document(mongodb::options::ReturnDocument::After)
                    .upsert(*upsert)
                    .session(session)
                    .await
            }
        }
    }
}

/// Fence reads and their resulting mutation share a snapshot transaction.
/// Unknown draft placeholders retain their existing compatibility behavior.
pub(super) async fn transact(
    client: &Client,
    db: &Database,
    pins: &mut Pins,
    mutation: &Mutation,
) -> Result<Option<Document>, Error> {
    let mut session = client.start_session().await.map_err(Error::Driver)?;
    transact_in(&mut session, db, pins, mutation).await
}

/// Runs one admission attempt in a caller-owned session, so a caller can
/// reconcile an unknown commit with a causal read in that same session.
pub(super) async fn transact_in(
    session: &mut mongodb::ClientSession,
    db: &Database,
    pins: &mut Pins,
    mutation: &Mutation,
) -> Result<Option<Document>, Error> {
    let session = &mut *session;
    start_transaction(session).await.map_err(Error::Driver)?;
    match pins.admit(db, session).await {
        Ok(true) => {}
        Ok(false) => {
            let _ = session.abort_guarded().await;
            return Err(Error::ReferenceUnavailable);
        }
        Err(error) => {
            let _ = session.abort_guarded().await;
            return Err(Error::Driver(error));
        }
    }
    let result = match mutation.apply(session).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            let _ = session.abort_guarded().await;
            return Ok(None);
        }
        Err(error) => {
            let _ = session.abort_guarded().await;
            return Err(Error::Driver(error));
        }
    };
    match commit_transaction(session).await {
        CommitOutcome::Committed => Ok(Some(result)),
        CommitOutcome::Retry(error)
        | CommitOutcome::Unknown(error)
        | CommitOutcome::Failed(error) => {
            mutation.retire_failed_commit(session).await;
            Err(Error::Driver(error))
        }
    }
}

pub(super) fn profile_error(error: Error) -> layer2_core::profiles::ProfileStoreError {
    use layer2_core::profiles::ProfileStoreError;
    match error {
        Error::ReferenceUnavailable => ProfileStoreError::ReferenceUnavailable,
        Error::Driver(error) if super::sandbox_mongo::is_duplicate_key(&error) => {
            ProfileStoreError::Duplicate
        }
        Error::Driver(error) if is_transaction_conflict(&error) => ProfileStoreError::Conflict,
        Error::Driver(error) => ProfileStoreError::Unavailable(error.to_string()),
    }
}

pub(super) fn is_transaction_conflict(error: &mongodb::error::Error) -> bool {
    if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) {
        return true;
    }
    match error.kind.as_ref() {
        ErrorKind::Command(command) => command.code == WRITE_CONFLICT,
        ErrorKind::Write(WriteFailure::WriteError(write)) => write.code == WRITE_CONFLICT,
        _ => false,
    }
}
