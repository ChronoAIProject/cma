use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome,
};
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(super) async fn claim_due(
    store: &MongoAccountDeletionStore,
    holder: &str,
    now: &str,
    lease: Duration,
) -> Result<Option<AccountDeletion>, AccountDeletionError> {
    for _ in 0..8 {
        let mut cursor=store.records.find(doc!{
            "status":"running","work.ready_at":{"$lte":now},
            "$or":[{"work.lease":{"$exists":false}},{"work.lease":mongodb::bson::Bson::Null},{"work.lease.expires_at":{"$lte":now}}],
        }).sort(doc!{"work.ready_at":1,"_id":1}).await.map_err(driver)?;
        let mut candidate = None;
        while let Some(row) = cursor.try_next().await.map_err(driver)? {
            let record = records::request_keys(&row).and_then(|_| read_record(row));
            match record {
                Ok(record) => {
                    candidate = Some(record);
                    break;
                }
                Err(error) => tracing::warn!(%error, "skipping unreadable account deletion claim"),
            }
        }
        let Some(mut record) = candidate else {
            return Ok(None);
        };
        let expected = record.revision;
        record.claim(holder, now, lease)?;
        let event = record.current_event(now, ErasureEventKind::Started);
        match commit(
            store,
            record.clone(),
            vec![event],
            expected,
            Some(now),
            None,
        )
        .await
        {
            Ok(()) => return Ok(Some(record)),
            Err(AccountDeletionError::Conflict) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(AccountDeletionError::Conflict)
}
pub(super) async fn commit(
    store: &MongoAccountDeletionStore,
    deletion: AccountDeletion,
    events: Vec<ErasureEvent>,
    expected: u64,
    claim_at: Option<&str>,
    expected_lease_token: Option<&str>,
) -> Result<(), AccountDeletionError> {
    for attempt in 0..8 {
        backoff(attempt, expected).await;
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
        match once(
            store,
            &deletion,
            &events,
            expected,
            claim_at,
            expected_lease_token,
            &mut session,
        )
        .await
        {
            Ok(()) => {}
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
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) => return Err(AccountDeletionError::RecoveryRequired),
            CommitOutcome::Failed(error) => return Err(driver(error)),
        }
    }
    Err(AccountDeletionError::Conflict)
}
async fn once(
    store: &MongoAccountDeletionStore,
    deletion: &AccountDeletion,
    events: &[ErasureEvent],
    expected: u64,
    claim_at: Option<&str>,
    expected_lease_token: Option<&str>,
    session: &mut mongodb::ClientSession,
) -> Result<(), AttemptError> {
    let row = store
        .records
        .find_one(doc! {"_id":deletion.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(AttemptError::Driver)?
        .ok_or(AttemptError::Domain(AccountDeletionError::NotFound))?;
    let old = read_record(row.clone()).map_err(AttemptError::Domain)?;
    let claimed = claim_at.is_some();
    let expected_next = old
        .work
        .next_fence
        .checked_add(u64::from(claimed))
        .ok_or(AttemptError::Domain(AccountDeletionError::Invalid))?;
    if old.revision != expected
        || deletion.revision <= expected
        || deletion.work.next_fence != expected_next
        || claim_at.is_some_and(|at| !old.is_due(at))
    {
        return Err(AttemptError::Domain(AccountDeletionError::Conflict));
    }
    let old_lease_token = old.work.lease.as_ref().map(|lease| lease.token.as_str());
    if !claimed && old_lease_token != expected_lease_token {
        return Err(AttemptError::Domain(AccountDeletionError::StaleFence));
    }
    if !claimed
        && deletion.work.lease.as_ref().is_some_and(|lease| {
            Some(lease.token.as_str()) != expected_lease_token
                || old
                    .work
                    .lease
                    .as_ref()
                    .is_none_or(|old| old.fence != lease.fence)
        })
    {
        return Err(AttemptError::Domain(AccountDeletionError::StaleFence));
    }
    if old.event_seq.checked_add(events.len() as u64) != Some(deletion.event_seq)
        || events.iter().enumerate().any(|(index, event)| {
            event.deletion_id != deletion.id || event.seq != old.event_seq + index as u64 + 1
        })
    {
        return Err(AttemptError::Domain(AccountDeletionError::Invalid));
    }
    let keys = records::request_keys(&row).map_err(AttemptError::Domain)?;
    let mut filter = doc! {"_id":deletion.id.as_str(),"revision":i64::try_from(expected).map_err(|_|AttemptError::Domain(AccountDeletionError::Invalid))?,"work.next_fence":i64::try_from(old.work.next_fence).map_err(|_|AttemptError::Domain(AccountDeletionError::Invalid))?};
    let held_token = if claimed {
        old_lease_token
    } else {
        expected_lease_token
    };
    if let Some(token) = held_token {
        filter.insert("work.lease.token", token);
    } else {
        filter.insert("work.lease", mongodb::bson::Bson::Null);
    }
    let result = store
        .records
        .replace_one(
            filter,
            record_document(deletion, &keys).map_err(AttemptError::Domain)?,
        )
        .session(&mut *session)
        .await
        .map_err(AttemptError::Driver)?;
    if result.matched_count != 1 {
        return Err(AttemptError::Domain(AccountDeletionError::Conflict));
    }
    if !events.is_empty() {
        let rows = events
            .iter()
            .map(|event| event_document(event, deletion.expire_at.as_deref()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(AttemptError::Domain)?;
        store
            .events
            .insert_many(rows)
            .session(&mut *session)
            .await
            .map_err(AttemptError::Driver)?;
    }
    if let Some(expiry) = &deletion.expire_at {
        store
            .events
            .update_many(
                doc! {"deletion_id":deletion.id.as_str()},
                doc! {"$set":{"expire_at":records::date(expiry).map_err(AttemptError::Domain)?}},
            )
            .session(session)
            .await
            .map_err(AttemptError::Driver)?;
    }
    Ok(())
}
enum AttemptError {
    Domain(AccountDeletionError),
    Driver(mongodb::error::Error),
}
