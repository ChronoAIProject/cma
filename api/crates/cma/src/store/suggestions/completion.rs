//! Publish one retained model result across known-aborted metadata transactions.
use super::*;
use crate::store::sandbox_mongo::TRANSACTION_ATTEMPTS;
use crate::store::transaction_terminal::GuardedTerminal as _;
use context::ContextError;

pub(super) async fn publish(
    store: &MongoSandboxStore,
    claim: &SuggestionWork,
    completion: &SuggestionCompletion,
) -> Result<(), AgentReadError> {
    for attempt in 0..TRANSACTION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
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
        match apply(store, claim, completion, &mut session).await {
            Ok(false) => {
                session.abort_guarded().await.map_err(unavailable)?;
                return Ok(());
            }
            Ok(true) => {}
            Err(ContextError::Driver(error)) if confirmed_transient(&error) => {
                let _ = session.abort_guarded().await;
                continue;
            }
            Err(error) => {
                let _ = session.abort_guarded().await;
                return Err(error.into_read_error());
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => continue,
            // An uncertain commit cannot authorize a new transaction. The
            // existing terminal claim semantics also prevent model redispatch.
            CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                return Err(unavailable(error));
            }
        }
    }
    Err(unavailable("suggestion completion retry budget exhausted"))
}

async fn apply(
    store: &MongoSandboxStore,
    claim: &SuggestionWork,
    completion: &SuggestionCompletion,
    session: &mut ClientSession,
) -> Result<bool, ContextError> {
    let row = store
        .suggestions
        .find_one(doc! {"_id": &claim.id})
        .session(&mut *session)
        .await
        .map_err(ContextError::Driver)?
        .ok_or(ContextError::Domain(AgentReadError::NotFound))?;
    let mut work = decode(&row).map_err(ContextError::Domain)?;
    // Retain the exact generated-for claim, including scope and context. A
    // newer claim or terminal result is supersession, never a retry invitation.
    if work.status != SuggestionStatus::Running || work != *claim {
        return Ok(false);
    }
    let parent = context::current_parent(store, &work, session, false).await?;
    if let Some(parent) = &parent {
        parent::fence(store, &work, parent, &mut *session).await?;
    }
    store
        .agents
        .update_one(
            doc! {"_id":work.agent_id.as_str()},
            doc! {"$inc":{"suggestion_completion_fence":1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(ContextError::Driver)?;
    work.complete(
        parent.is_some(),
        SuggestionCompletion {
            status: completion.status,
            set: completion.set.clone(),
            observation: completion.observation.clone(),
        },
    );
    store
        .suggestions
        .replace_one(
            doc! {"_id": &work.id},
            document(&work).map_err(ContextError::Domain)?,
        )
        .session(session)
        .await
        .map_err(ContextError::Driver)?;
    Ok(true)
}
