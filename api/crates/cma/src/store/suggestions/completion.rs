//! Publish one retained model result across known-aborted metadata transactions.
use super::*;
use crate::store::sandbox_mongo::{is_transient, TRANSACTION_ATTEMPTS};

enum ApplyError {
    Domain(AgentReadError),
    Driver(mongodb::error::Error),
}

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
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(unavailable(error)),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err(unavailable(error));
        }
        match apply(store, claim, completion, &mut session).await {
            Ok(false) => {
                let _ = session.abort_transaction().await;
                return Ok(());
            }
            Ok(true) => {}
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                continue;
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                return Err(match error {
                    ApplyError::Domain(error) => error,
                    ApplyError::Driver(error) => unavailable(error),
                });
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
) -> Result<bool, ApplyError> {
    let row = store
        .suggestions
        .find_one(doc! {"_id": &claim.id})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(AgentReadError::NotFound))?;
    let mut work = decode(&row).map_err(ApplyError::Domain)?;
    // Retain the exact generated-for claim, including scope and context. A
    // newer claim or terminal result is supersession, never a retry invitation.
    if work.status != SuggestionStatus::Running || work != *claim {
        return Ok(false);
    }
    let current = context::current(store, &work, session, false)
        .await
        .map_err(ApplyError::Domain)?;
    store
        .agents
        .update_one(
            doc! {"_id":work.agent_id.as_str()},
            doc! {"$inc":{"suggestion_completion_fence":1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    work.complete(
        current,
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
            document(&work).map_err(ApplyError::Domain)?,
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    Ok(true)
}
