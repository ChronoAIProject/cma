use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::ids::SubjectId;
use layer2_core::trigger_domain::{
    PublicTrigger, TriggerId, TriggerReferenceError, TriggerStoreError,
};
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};
use super::super::schema::DOCUMENT_SCHEMA_VERSION;
use super::documents::{read_trigger, revision_document, trigger_document};
use super::profile_resolution::{self, TransactionProfileError};
use super::queries::{self, TransactionReadError};
use super::MongoTriggerDefinitionStore;

pub(super) async fn publish(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    owner: &SubjectId,
    expected: u64,
    published_at: String,
) -> Result<PublicTrigger, TriggerStoreError> {
    let mut last_transient = None;
    for attempt_number in 0..TRANSACTION_ATTEMPTS {
        if attempt_number > 0 {
            tokio::task::yield_now().await;
        }
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(unavailable("start Trigger publication transaction", error)),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_transient = Some(error.to_string());
                continue;
            }
            return Err(unavailable("begin Trigger publication transaction", error));
        }
        let published =
            match publish_once(store, id, owner, expected, &published_at, &mut session).await {
                Ok(published) => published,
                Err(AttemptError::Retry(reason)) => {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(reason.into());
                    continue;
                }
                Err(AttemptError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(AttemptError::Driver(error))
                    if is_transient(&error) || is_duplicate_key(&error) =>
                {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(AttemptError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(unavailable("write Trigger publication", error));
                }
            };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(published),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                return reconcile_unknown(store, id, &published, error).await;
            }
            CommitOutcome::Failed(error) => {
                return Err(unavailable("commit Trigger publication", error));
            }
        }
    }
    Err(TriggerStoreError::Unavailable(format!(
        "Trigger publication exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

async fn publish_once(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    owner: &SubjectId,
    expected: u64,
    published_at: &str,
    session: &mut ClientSession,
) -> Result<PublicTrigger, AttemptError> {
    let document = store
        .definitions
        .find_one(doc! { "_id": id.as_str(), "owner": owner.as_str(), "archived_at": mongodb::bson::Bson::Null })
        .session(&mut *session)
        .await
        .map_err(AttemptError::Driver)?
        .ok_or(AttemptError::Domain(TriggerStoreError::NotFound))?;
    if document.get_bool("account_erasure_fenced").ok() == Some(true) {
        return Err(AttemptError::Domain(
            TriggerReferenceError::Unavailable {
                field: "trigger publisher",
            }
            .into(),
        ));
    }
    let stored = read_trigger(&document).map_err(AttemptError::Domain)?;
    match queries::verify_in_session(store, &stored.trigger, session).await {
        Ok(()) => {}
        Err(TransactionReadError::Domain(error)) => return Err(AttemptError::Domain(error)),
        Err(TransactionReadError::Driver(error)) => return Err(AttemptError::Driver(error)),
    }
    if stored.trigger.draft_version != expected {
        return Err(AttemptError::Domain(TriggerStoreError::Conflict {
            expected,
            actual: stored.trigger.draft_version,
        }));
    }
    match profile_resolution::authorize(store, owner, &stored.trigger.draft.target, session).await {
        Ok(_) => {}
        Err(TransactionProfileError::Domain(error)) => {
            return Err(AttemptError::Domain(error));
        }
        Err(TransactionProfileError::Driver(error)) => {
            return Err(AttemptError::Driver(error));
        }
    }

    let mut changed = stored.trigger;
    let revision = changed
        .publish(published_at.to_owned())
        .map_err(TriggerStoreError::from)
        .map_err(AttemptError::Domain)?;
    let public_id = changed.public_id.clone().ok_or_else(|| {
        AttemptError::Domain(TriggerStoreError::Corrupt(
            "published Trigger has no public ID".into(),
        ))
    })?;
    let next_mutation = stored.mutation_version.checked_add(1).ok_or_else(|| {
        AttemptError::Domain(TriggerStoreError::Corrupt(
            "Trigger mutation version is exhausted".into(),
        ))
    })?;
    store
        .revisions
        .insert_one(revision_document(&changed, &revision).map_err(AttemptError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(AttemptError::Driver)?;
    let result = store
        .definitions
        .replace_one(
            doc! {
                "_id": id.as_str(),
                "owner": owner.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "account_erasure_fenced": { "$ne": true },
                "draft_version": signed(expected, "expected Trigger draft version")
                    .map_err(TriggerStoreError::Corrupt)
                    .map_err(AttemptError::Domain)?,
                "mutation_version": signed(stored.mutation_version, "Trigger mutation version")
                    .map_err(TriggerStoreError::Corrupt)
                    .map_err(AttemptError::Domain)?,
            },
            trigger_document(&changed, next_mutation).map_err(AttemptError::Domain)?,
        )
        .session(session)
        .await
        .map_err(AttemptError::Driver)?;
    if result.matched_count != 1 {
        return Err(AttemptError::Retry(
            "Trigger definition changed during publication",
        ));
    }
    Ok(PublicTrigger {
        trigger_id: changed.id,
        public_id,
        owner: changed.owner,
        availability: changed.availability,
        revision,
    })
}

async fn reconcile_unknown(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    intended: &PublicTrigger,
    error: mongodb::error::Error,
) -> Result<PublicTrigger, TriggerStoreError> {
    if let Some(stored) = queries::by_id(store, id).await? {
        let committed = stored.trigger.public_id.as_ref() == Some(&intended.public_id)
            && stored.trigger.owner == intended.owner
            && stored
                .trigger
                .revisions
                .iter()
                .any(|revision| revision == &intended.revision);
        if committed {
            return Ok(intended.clone());
        }
    }
    Err(TriggerStoreError::Unavailable(format!(
        "Trigger publication commit remained unknown: {error}"
    )))
}

enum AttemptError {
    Retry(&'static str),
    Domain(TriggerStoreError),
    Driver(mongodb::error::Error),
}

fn unavailable(context: &str, error: mongodb::error::Error) -> TriggerStoreError {
    TriggerStoreError::Unavailable(format!("{context}: {error}"))
}
