//! Shared transaction isolation, error classification and bounded commit retries.
use mongodb::error::{
    Error, ErrorKind, WriteFailure, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT,
};
use mongodb::options::{ReadConcern, WriteConcern};
use mongodb::ClientSession;

const COMMIT_ATTEMPTS: usize = 5;
const DUPLICATE_KEY: i32 = 11_000;

pub(crate) fn is_duplicate_key(error: &Error) -> bool {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write)) => write.code == DUPLICATE_KEY,
        ErrorKind::Command(command) => command.code == DUPLICATE_KEY,
        _ => false,
    }
}

pub(crate) fn is_transient(error: &Error) -> bool {
    error.contains_label(TRANSIENT_TRANSACTION_ERROR)
}

pub(crate) enum CommitOutcome {
    Committed,
    Retry(Error),
    Unknown(Error),
    Failed(Error),
}

pub(crate) async fn start_transaction(session: &mut ClientSession) -> Result<(), Error> {
    session
        .start_transaction()
        .read_concern(ReadConcern::snapshot())
        .write_concern(WriteConcern::majority())
        .await
}

pub(crate) async fn commit_transaction(session: &mut ClientSession) -> CommitOutcome {
    for attempt in 0..COMMIT_ATTEMPTS {
        match session.commit_transaction().await {
            Ok(()) => return CommitOutcome::Committed,
            Err(error) if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) => {
                if attempt + 1 == COMMIT_ATTEMPTS {
                    return CommitOutcome::Unknown(error);
                }
                tokio::task::yield_now().await;
            }
            Err(error) if is_transient(&error) => return CommitOutcome::Retry(error),
            Err(error) => return CommitOutcome::Failed(error),
        }
    }
    unreachable!("the bounded commit loop always returns")
}
