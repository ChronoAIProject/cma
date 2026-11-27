//! Claim a pending suggestion before model dispatch using known-aborted retries.
use super::*;
use crate::store::sandbox_mongo::TRANSACTION_ATTEMPTS;
use crate::store::transaction_terminal::GuardedTerminal as _;
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
    if let Some(control) = &store.suggestion_control {
        control.exclude_held_expiry(&mut filter, now_ms).await;
    }
    let mut selected = None;
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if confirmed_transient(&error) => continue,
            Err(error) => return Err(unavailable(error)),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if confirmed_transient(&error) {
                continue;
            }
            return Err(unavailable(error));
        }
        let (work, claimed) = match apply(store, &filter, now_ms, &mut selected, &mut session).await
        {
            Ok(None) => {
                session.abort_guarded().await.map_err(unavailable)?;
                return Ok(None);
            }
            Ok(Some(updated)) => updated,
            Err(ContextError::Driver(error)) if confirmed_transient(&error) => {
                let _ = session.abort_guarded().await;
                continue;
            }
            Err(error) => {
                let _ = session.abort_guarded().await;
                return Err(error.into_read_error());
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(claimed.then_some(work)),
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
    selected: &mut Option<String>,
    session: &mut ClientSession,
) -> Result<Option<(SuggestionWork, bool)>, ContextError> {
    let mut filter = filter.clone();
    if let Some(id) = selected.as_ref() {
        filter.insert("_id", id);
    }
    let Some(row) = store
        .suggestions
        .find_one(filter)
        .sort(doc! {"created_at_ms":1,"_id":1})
        .session(&mut *session)
        .await
        .map_err(ContextError::Driver)?
    else {
        return Ok(None);
    };
    let mut work = decode(&row).map_err(ContextError::Domain)?;
    selected.get_or_insert_with(|| work.id.clone());
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
    Ok(Some((work, claimed)))
}
