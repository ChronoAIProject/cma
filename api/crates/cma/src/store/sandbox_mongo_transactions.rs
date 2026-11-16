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
    // Uncertainty cannot authorize a fresh transaction, even if a server or
    // proxy also attaches the known-abort label to the same response.
    !error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT)
        && error.contains_label(TRANSIENT_TRANSACTION_ERROR)
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
        .await?;
    if let Err(error) = crate::store::work_publication::fence_current(session).await {
        if let Err(abort) = session.abort_transaction().await {
            tracing::warn!(error = %abort, "ordinary work transaction abort was not acknowledged");
        }
        return Err(error);
    }
    Ok(())
}

pub(in crate::store) async fn commit_transaction(session: &mut ClientSession) -> CommitOutcome {
    if let Err(error) = commit_authority() {
        if let Err(abort) = session.abort_transaction().await {
            tracing::warn!(error = %abort, "expired work publication abort was not acknowledged");
        }
        return CommitOutcome::Failed(Error::custom(error));
    }
    commit_with(session).await
}

/// Only `Aborted` proves that custody may be released without a commit send.
pub(in crate::store) enum PrecommitRefusal {
    Aborted(layer2_core::sandbox::store::SandboxStoreError),
    AbortUnacknowledged(Error),
}

fn commit_authority() -> Result<(), layer2_core::sandbox::store::SandboxStoreError> {
    #[cfg(test)]
    let _ = precommit_probe::BEFORE_GUARD.try_with(|hook| hook());
    layer2_core::sandbox::store::WorkAuthority::current().map(|_| ())
}

/// Preserve precommit refusal separately from outcomes after a commit send.
/// The caller must retain custody for every unacknowledged abort.
pub(in crate::store) async fn commit_transaction_with_refusal(
    session: &mut ClientSession,
) -> Result<CommitOutcome, PrecommitRefusal> {
    if let Err(refusal) = commit_authority() {
        // The driver's abort_transaction() deliberately ignores command errors.
        // Observe the explicit command reply before allowing custody release.
        // Using this session keeps the original lsid, transaction number and pin.
        let reply = session
            .client()
            .database("admin")
            .run_command(mongodb::bson::doc! {"abortTransaction":1_i32})
            .session(&mut *session)
            .await;
        // Clear the driver's local transaction state even when the first reply
        // was lost. Its best-effort abort is never acknowledgment evidence.
        if let Err(error) = session.abort_transaction().await {
            tracing::warn!(error = %error, "precommit refusal could not close local transaction state");
        }
        return Err(match reply {
            Ok(reply)
                if !reply.contains_key("writeConcernError")
                    && !reply.contains_key("errorLabels") =>
            {
                PrecommitRefusal::Aborted(refusal)
            }
            Ok(_) => PrecommitRefusal::AbortUnacknowledged(Error::custom(
                "precommit abort acknowledgment contains uncertainty",
            )),
            Err(error) => PrecommitRefusal::AbortUnacknowledged(error),
        });
    }
    Ok(commit_with(session).await)
}

#[cfg(test)]
pub(in crate::store) mod precommit_probe {
    tokio::task_local! {
        pub(in crate::store) static BEFORE_GUARD: std::sync::Arc<dyn Fn() + Send + Sync>;
    }
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
