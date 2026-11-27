//! Agent Manager transaction isolation, publication fencing and terminal commands.
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::transaction_terminal::KillOnDrop;
use cma_am_mongo_primitives::transactions::{
    commit_with, CommitFailure, CommitOperation, CommitResolution,
};
use mongodb::error::Error;
use mongodb::options::{ReadConcern, WriteConcern};
use mongodb::ClientSession;

pub use cma_am_mongo_primitives::transactions::{
    is_confirmed_body_retry, is_duplicate_key, is_transient,
};

pub(crate) type CommitOutcome = CommitResolution<Error>;

struct GuardedCommit<'a>(&'a mut ClientSession);

impl CommitOperation for GuardedCommit<'_> {
    type Error = Error;

    async fn commit(&mut self) -> Result<(), CommitFailure<Error>> {
        self.0.commit_guarded().await.map_err(CommitFailure::from)
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
    commit_with(&mut GuardedCommit(session)).await
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
        return Err(match abort_with_acknowledgment(session).await {
            Ok(()) => PrecommitRefusal::Aborted(refusal),
            Err(error) => PrecommitRefusal::AbortUnacknowledged(error),
        });
    }
    Ok(commit_with(&mut GuardedCommit(session)).await)
}

pub(in crate::store) async fn abort_with_acknowledgment(
    session: &mut ClientSession,
) -> Result<(), Error> {
    // The driver's convenience abort suppresses command failures. Observe the
    // reply on the same session and pinned route, then close local driver state.
    let guard = KillOnDrop::arm(session);
    let reply = session
        .client()
        .database("admin")
        .run_command(mongodb::bson::doc! {"abortTransaction":1_i32})
        .session(&mut *session)
        .await;
    guard.disarm();
    let _ = session.abort_guarded().await;
    if acknowledged_abort_reply(&reply?) {
        return Ok(());
    }
    Err(Error::custom("precommit abort acknowledgment is uncertain"))
}

fn acknowledged_abort_reply(reply: &mongodb::bson::Document) -> bool {
    use mongodb::bson::Bson;
    !reply.contains_key("writeConcernError")
        && !reply.contains_key("errorLabels")
        && (matches!(reply.get("ok"), Some(Bson::Int32(1) | Bson::Int64(1)))
            || matches!(reply.get("ok"), Some(Bson::Double(value)) if *value == 1.0))
}

#[cfg(test)]
#[path = "sandbox_mongo_transactions_tests.rs"]
mod tests;
