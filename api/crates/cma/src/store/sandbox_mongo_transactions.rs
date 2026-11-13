//! Shared transaction isolation, error classification and bounded commit retries.
use mongodb::error::{
    Error, ErrorKind, WriteFailure, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT,
};
use mongodb::options::{ReadConcern, WriteConcern};
use mongodb::ClientSession;

const COMMIT_ATTEMPTS: usize = 5;
const DUPLICATE_KEY: i32 = 11_000;

pub(in crate::store) fn is_duplicate_key(error: &Error) -> bool {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write)) => write.code == DUPLICATE_KEY,
        ErrorKind::Command(command) => command.code == DUPLICATE_KEY,
        _ => false,
    }
}

pub(in crate::store) fn is_transient(error: &Error) -> bool {
    error.contains_label(TRANSIENT_TRANSACTION_ERROR)
}

pub(in crate::store) enum CommitResolution<E> {
    Committed,
    Retry(E),
    Unknown(E),
    Failed(E),
}

pub(in crate::store) type CommitOutcome = CommitResolution<Error>;

enum CommitFailure<E> {
    Unknown(E),
    Transient(E),
    Failed(E),
}

#[async_trait::async_trait]
trait CommitOperation: Send {
    type Error: Send;
    async fn commit(&mut self) -> Result<(), CommitFailure<Self::Error>>;
}

#[async_trait::async_trait]
impl CommitOperation for ClientSession {
    type Error = Error;

    async fn commit(&mut self) -> Result<(), CommitFailure<Error>> {
        self.commit_transaction().await.map_err(|error| {
            if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) {
                CommitFailure::Unknown(error)
            } else if is_transient(&error) {
                CommitFailure::Transient(error)
            } else {
                CommitFailure::Failed(error)
            }
        })
    }
}

pub(in crate::store) async fn start_transaction(session: &mut ClientSession) -> Result<(), Error> {
    session
        .start_transaction()
        .read_concern(ReadConcern::snapshot())
        .write_concern(WriteConcern::majority())
        .await
}

pub(in crate::store) async fn commit_transaction(session: &mut ClientSession) -> CommitOutcome {
    commit_with(session).await
}

async fn commit_with<C: CommitOperation>(commit: &mut C) -> CommitResolution<C::Error> {
    let mut uncertain = false;
    for attempt in 0..COMMIT_ATTEMPTS {
        match commit.commit().await {
            Ok(()) => return CommitResolution::Committed,
            Err(CommitFailure::Unknown(error)) => {
                uncertain = true;
                if attempt + 1 == COMMIT_ATTEMPTS {
                    return CommitResolution::Unknown(error);
                }
                tokio::task::yield_now().await;
            }
            // A later response cannot prove that an earlier uncertain commit
            // aborted. Retain uncertainty instead of authorizing mutation replay.
            Err(CommitFailure::Transient(error) | CommitFailure::Failed(error)) if uncertain => {
                return CommitResolution::Unknown(error);
            }
            Err(CommitFailure::Transient(error)) => return CommitResolution::Retry(error),
            Err(CommitFailure::Failed(error)) => return CommitResolution::Failed(error),
        }
    }
    unreachable!("the bounded commit loop always returns")
}

#[cfg(test)]
#[path = "sandbox_mongo_transactions_tests.rs"]
mod tests;
