use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
};
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(super) async fn request(
    store: &MongoAccountDeletionStore,
    subject: &SubjectId,
    key: &str,
    at: &str,
) -> Result<AccountDeletion, AccountDeletionError> {
    let initial = AccountDeletion::request(AccountDeletionId::generate(), subject.clone(), at)?;
    // Establish only the pre-existing fence shape before competing transactional
    // upserts. Every accepted deletion still increments it in its transaction.
    let seed=store.database.collection::<Document>("sandbox_admissions").update_one(
        doc!{"_id":subject.as_str()},doc!{"$setOnInsert":{"fence":0_i64,"document_schema_version":crate::store::schema::DOCUMENT_SCHEMA_VERSION}},
    ).upsert(true).await;
    match seed {
        Ok(_) => {}
        Err(error) if is_duplicate_key(&error) => {}
        Err(error) => return Err(driver(error)),
    }
    for attempt in 0..8 {
        backoff(attempt, initial.id.as_str().bytes().map(u64::from).sum()).await;
        let mut session = store
            .database
            .client()
            .start_session()
            .await
            .map_err(driver)?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err(driver(error));
        }
        let result = once(store, subject, key, &initial, &mut session).await;
        let record = match result {
            Ok(record) => record,
            Err(AttemptError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(AttemptError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                if is_transient(&error) {
                    continue;
                }
                return Err(driver(error));
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(record),
            CommitOutcome::Retry(_) => continue,
            // An uncertain commit is reconciled only by its durable request key.
            CommitOutcome::Unknown(_) => return Err(AccountDeletionError::RecoveryRequired),
            CommitOutcome::Failed(error) => return Err(driver(error)),
        }
    }
    Err(AccountDeletionError::Store(
        "account deletion admission contention exceeded the retry budget".into(),
    ))
}
async fn once(
    store: &MongoAccountDeletionStore,
    subject: &SubjectId,
    key: &str,
    initial: &AccountDeletion,
    session: &mut mongodb::ClientSession,
) -> Result<AccountDeletion, AttemptError> {
    fence_subject_in_session(&store.database, subject, session)
        .await
        .map_err(AttemptError::Driver)?;
    let hash = subject_hash(subject);
    let key = key_hash(key);
    if let Some(row) = store
        .records
        .find_one(doc! {"subject_hash":&hash,"request_keys":&key})
        .session(&mut *session)
        .await
        .map_err(AttemptError::Driver)?
    {
        return read_record(row).map_err(AttemptError::Domain);
    }
    if let Some(row) = store
        .records
        .find_one(doc! {"subject_hash":hash,"status":{"$ne":"completed"}})
        .session(&mut *session)
        .await
        .map_err(AttemptError::Driver)?
    {
        let record = read_record(row).map_err(AttemptError::Domain)?;
        return Ok(record);
    }
    store
        .records
        .insert_one(record_document(initial, &[key]).map_err(AttemptError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(AttemptError::Driver)?;
    let event = initial.current_event(&initial.requested_at, ErasureEventKind::Started);
    store
        .events
        .insert_one(event_document(&event, None).map_err(AttemptError::Domain)?)
        .session(session)
        .await
        .map_err(AttemptError::Driver)?;
    Ok(initial.clone())
}
enum AttemptError {
    Domain(AccountDeletionError),
    Driver(mongodb::error::Error),
}
