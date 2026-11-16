//! Claim a pending suggestion before model dispatch using known-aborted retries.
use super::*;
use crate::store::sandbox_mongo::{is_transient, TRANSACTION_ATTEMPTS};
use context::ContextError;

pub(super) async fn acquire(
    store: &MongoSandboxStore,
    now_ms: u64,
) -> Result<Option<SuggestionWork>, AgentReadError> {
    let mut filter = doc! {"version":STRATEGY_VERSION,"$or": [
        {"status":"pending"}, {"status":"running", "deadline_ms":{"$lte": now_ms as i64}}
    ]};
    if let Some(accounts) = store
        .writer_policy
        .writer_accounts(TaskExperienceWriter::Suggestions)
    {
        if accounts.is_empty() {
            return Ok(None);
        }
        filter.insert("owner", doc! {"$in":accounts});
    }
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(unavailable(error)),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err(unavailable(error));
        }
        let claim = match apply(store, &filter, now_ms, &mut session).await {
            Ok(claim) => claim,
            Err(ContextError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                continue;
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                return Err(match error {
                    ContextError::Driver(error) => unavailable(error),
                    ContextError::Domain(error) => error,
                });
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(claim),
            CommitOutcome::Retry(_) => continue,
            // An unacknowledged claim is never permission to run the model or
            // claim another item. Retain its existing timeout/custody semantics.
            CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                return Err(unavailable(error));
            }
        }
    }
    Err(unavailable("suggestion claim retry budget exhausted"))
}

async fn apply(
    store: &MongoSandboxStore,
    filter: &Document,
    now_ms: u64,
    session: &mut ClientSession,
) -> Result<Option<SuggestionWork>, ContextError> {
    let Some(row) = store
        .suggestions
        .find_one(filter.clone())
        .sort(doc! {"created_at_ms":1,"_id":1})
        .session(&mut *session)
        .await
        .map_err(ContextError::Driver)?
    else {
        return Ok(None);
    };
    let mut work = decode(&row).map_err(ContextError::Domain)?;
    let current = context::current_transaction(store, &work, session, false).await?;
    let claimed = work.claim(current, now_ms);
    store
        .suggestions
        .replace_one(
            doc! {"_id":&work.id},
            document(&work).map_err(ContextError::Domain)?,
        )
        .session(session)
        .await
        .map_err(ContextError::Driver)?;
    Ok(claimed.then_some(work))
}
