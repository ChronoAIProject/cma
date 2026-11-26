//! Classify MongoDB transaction outcomes and retry an uncertain commit on the same operation.
use mongodb::error::{
    Error, ErrorKind, WriteFailure, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT,
};

const COMMIT_ATTEMPTS: usize = 5;
const DUPLICATE_KEY: i32 = 11_000;

pub fn is_duplicate_key(error: &Error) -> bool {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write)) => write.code == DUPLICATE_KEY,
        ErrorKind::Command(command) => command.code == DUPLICATE_KEY,
        _ => false,
    }
}

pub fn is_transient(error: &Error) -> bool {
    // Only a known abort permits a fresh mutation. Multiple labels do not
    // erase uncertainty about an earlier transaction result.
    !error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT)
        && error.contains_label(TRANSIENT_TRANSACTION_ERROR)
}

/// A body error with uncertain outcome never permits a new transaction.
pub fn is_confirmed_body_retry(error: &Error) -> bool {
    !error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT)
        && (is_transient(error) || is_duplicate_key(error))
}

pub enum CommitResolution<E> {
    Committed,
    Retry(E),
    Unknown(E),
    Failed(E),
}

pub type CommitOutcome = CommitResolution<Error>;

pub enum CommitFailure<E> {
    Unknown(E),
    Transient(E),
    Failed(E),
}

#[async_trait::async_trait]
pub trait CommitOperation: Send {
    type Error: Send;
    async fn commit(&mut self) -> Result<(), CommitFailure<Self::Error>>;
}

/// Classify one commit response, preserving uncertainty when both labels are present.
pub fn classify_commit_error(error: Error) -> CommitFailure<Error> {
    if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) {
        CommitFailure::Unknown(error)
    } else if is_transient(&error) {
        CommitFailure::Transient(error)
    } else {
        CommitFailure::Failed(error)
    }
}

pub async fn commit_with<C: CommitOperation>(commit: &mut C) -> CommitResolution<C::Error> {
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
mod tests;
