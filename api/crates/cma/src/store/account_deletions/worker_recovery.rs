use super::worker::*;
use super::{worker_guard as guard, worker_records as records};
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome,
};
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(super) async fn resolve(
    store: &MongoErasureJournal,
    request: &JournalRequest,
    fence: Option<&JournalClaim>,
    budget: &JournalBudget,
) -> Result<JournalResolution, JournalError> {
    request.validate()?;
    let permit = tokio::time::timeout(budget.remaining()?, COMMITS.acquire())
        .await
        .map_err(|_| JournalError::Cancelled)?
        .map_err(|_| JournalError::Unavailable)?;
    tokio::time::timeout(budget.remaining()?, worker_schema::verify(&store.database))
        .await
        .map_err(|_| JournalError::Cancelled)??;
    for _ in 0..8 {
        budget.remaining()?;
        let mut session = tokio::time::timeout(
            budget.remaining()?,
            store
                .database
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
        let result = tokio::time::timeout(
            budget.remaining()?,
            ordered(store, request, fence, &mut session),
        )
        .await;
        let result = match result {
            Ok(Ok(value)) => value,
            other => {
                let _ = session.abort_guarded().await;
                match other {
                    Ok(Err(AttemptError::Driver(error))) if is_transient(&error) => continue,
                    Ok(Err(AttemptError::Domain(error))) => return Err(error),
                    Err(_) => return Err(JournalError::Cancelled),
                    _ => return Err(JournalError::Unavailable),
                }
            }
        };
        if !matches!(result, JournalResolution::NotCommittedAfterFence { .. }) {
            let _ = session.abort_guarded().await;
            return Ok(result);
        }
        let remaining = match budget.remaining() {
            Ok(value) => value,
            Err(error) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
        };
        let task = tokio::spawn(async move {
            let outcome = commit_transaction(&mut session).await;
            drop(permit);
            outcome
        });
        return match tokio::time::timeout(remaining, task).await {
            Ok(Ok(CommitOutcome::Committed)) if budget.remaining().is_ok() => Ok(result),
            _ => Ok(JournalResolution::Unresolved),
        };
    }
    Ok(JournalResolution::Unresolved)
}

async fn ordered(
    store: &MongoErasureJournal,
    request: &JournalRequest,
    fence: Option<&JournalClaim>,
    session: &mut ClientSession,
) -> Attempt<JournalResolution> {
    let (old, document, _) = guard::parent(store, &request.binding().deletion_id, session).await?;
    let row = store
        .checkpoints
        .find_one(doc! {"_id":old.id.as_str()})
        .session(&mut *session)
        .await?
        .map(records::checkpoint)
        .transpose()?;
    if let Some(receipt) =
        super::worker_journal::history(store, request, row.as_ref(), &old, session).await?
    {
        return Ok(JournalResolution::Committed(receipt));
    }
    let Some(fence) = fence else {
        return Ok(JournalResolution::Unresolved);
    };
    let mut row = row.ok_or(JournalError::Incompatible)?;
    guard::claim(&row, &old, fence)?;
    if row.checkpoint.binding.contract != request.binding().contract
        || row.id != request.binding().deletion_id
    {
        return Err(JournalError::BindingMismatch.into());
    }
    let mut next = old.clone();
    next.revision = next.revision.checked_add(1).ok_or(JournalError::Capacity)?;
    row.checkpoint.parent_revision = next.revision;
    let changed = store
        .checkpoints
        .replace_one(
            doc! {"_id":old.id.as_str(),"checkpoint.parent_revision":guard::signed(old.revision)?},
            records::encode(&row)?,
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(JournalError::Conflict.into());
    }
    // A successful write to the common parent orders this snapshot after any
    // conflicting predecessor. Absence without this boundary is never a result.
    guard::publish(store, &old, &document, &next, None, false, session).await?;
    if let Some(receipt) =
        super::worker_journal::history(store, request, Some(&row), &next, session).await?
    {
        return Ok(JournalResolution::Committed(receipt));
    }
    Ok(JournalResolution::NotCommittedAfterFence {
        operation: request.key().into(),
        claim: JournalClaim::from_record(row.checkpoint, &next)?,
    })
}
