//! MongoDB transaction labels and bounded resolution of uncertain commits.
use std::future::Future;

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

pub enum CommitFailure<E> {
    Unknown(E),
    Transient(E),
    Failed(E),
}

pub trait CommitOperation: Send {
    type Error: Send;
    fn commit(&mut self) -> impl Future<Output = Result<(), CommitFailure<Self::Error>>> + Send;
}

impl From<Error> for CommitFailure<Error> {
    fn from(error: Error) -> Self {
        if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) {
            Self::Unknown(error)
        } else if is_transient(&error) {
            Self::Transient(error)
        } else {
            Self::Failed(error)
        }
    }
}

/// Resolve a bounded sequence of commits on the same transaction.
/// The supplied operation owns the session and any cancellation guard.
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
#[path = "transactions_tests.rs"]
mod tests;
