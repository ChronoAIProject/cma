//! Shared transaction isolation, error classification and bounded commit retries.
use crate::store::transaction_terminal::GuardedTerminal as _;
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

pub(in crate::store) fn is_transient(error: &Error) -> bool {
    // Only a known abort permits a fresh mutation. Multiple labels do not
    // erase uncertainty about an earlier transaction result.
    !error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT)
        && error.contains_label(TRANSIENT_TRANSACTION_ERROR)
}

/// A body error with uncertain outcome never permits a new transaction.
pub(crate) fn is_confirmed_body_retry(error: &Error) -> bool {
    !error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT)
        && (is_transient(error) || is_duplicate_key(error))
}

pub(crate) enum CommitResolution<E> {
    Committed,
    Retry(E),
    Unknown(E),
    Failed(E),
}

pub(crate) type CommitOutcome = CommitResolution<Error>;

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
        self.commit_guarded().await.map_err(|error| {
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

pub(crate) async fn start_transaction(session: &mut ClientSession) -> Result<(), Error> {
    session
        .start_transaction()
        .read_concern(ReadConcern::snapshot())
        .write_concern(WriteConcern::majority())
        .await?;
    // Read before the work-publication fence's Sandbox or Workspace write.
    if let Err(error) = crate::store::transaction_barrier::open(session).await {
        if let Err(abort) = session.abort_guarded().await {
            tracing::warn!(error = %abort, "ordinary work transaction abort was not acknowledged");
        }
        return Err(error);
    }
    if let Err(error) = crate::store::work_publication::fence_current(session).await {
        if let Err(abort) = session.abort_guarded().await {
            tracing::warn!(error = %abort, "ordinary work transaction abort was not acknowledged");
        }
        return Err(error);
    }
    Ok(())
}

pub(crate) async fn commit_transaction(session: &mut ClientSession) -> CommitOutcome {
    if let Err(error) = layer2_core::sandbox::store::WorkAuthority::current() {
        if let Err(abort) = session.abort_guarded().await {
            tracing::warn!(error = %abort, "expired work publication abort was not acknowledged");
        }
        return CommitOutcome::Failed(Error::custom(error));
    }
    commit_with(session).await
}

/// Only an acknowledged abort before any commit send releases durable custody.
pub(in crate::store) enum PrecommitRefusal {
    Aborted(layer2_core::sandbox::store::SandboxStoreError),
    AbortUnacknowledged(Error),
}

pub(in crate::store) async fn commit_transaction_with_refusal(
    session: &mut ClientSession,
) -> Result<CommitOutcome, PrecommitRefusal> {
    if let Err(refusal) = layer2_core::sandbox::store::WorkAuthority::current() {
        // The driver's convenience abort suppresses command failures. Observe
        // the reply on this same session/transaction and pinned route first.
        let reply = session
            .client()
            .database("admin")
            .run_command(mongodb::bson::doc! {"abortTransaction":1_i32})
            .session(&mut *session)
            .await;
        // Close local driver state; this second best-effort abort proves nothing
        // about an uncertain first reply and cannot release the durable marker.
        if session.abort_guarded().await.is_err() {
            tracing::warn!(
                stage = "precommit_refusal",
                outcome = "local_abort_failed",
                "publication custody remains governed by the original abort reply"
            );
        }
        return Err(match reply {
            Ok(reply) if acknowledged_abort_reply(&reply) => PrecommitRefusal::Aborted(refusal),
            Ok(_) => PrecommitRefusal::AbortUnacknowledged(Error::custom(
                "precommit abort acknowledgment is uncertain",
            )),
            Err(error) => PrecommitRefusal::AbortUnacknowledged(error),
        });
    }
    Ok(commit_with(session).await)
}

fn acknowledged_abort_reply(reply: &mongodb::bson::Document) -> bool {
    use mongodb::bson::Bson;
    !reply.contains_key("writeConcernError")
        && !reply.contains_key("errorLabels")
        && (matches!(reply.get("ok"), Some(Bson::Int32(1) | Bson::Int64(1)))
            || matches!(reply.get("ok"), Some(Bson::Double(value)) if *value == 1.0))
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
