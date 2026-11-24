//! Cross-document capture publication shares one snapshot/majority transaction.
use super::*;
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::ClientSession;

fn canonical(value: &str) -> Result<u64, HomeBackupError> {
    layer2_core::seconds_from_rfc3339(value)
        .ok_or(HomeBackupError::Invalid("noncanonical timestamp"))
}
fn server_time(field: &str) -> Document {
    doc! {"$dateFromString":{"dateString":field,"onError":null,"onNull":null}}
}
fn live(lease: &HomeBackupLease, now: &str) -> Document {
    doc! {"lease.token":&lease.token,"lease.fence":codec::signed(lease.fence).expect("validated fence"),
    "$expr":{"$and":[
        {"$gt":[server_time("$lease.expires_at"),server_time(now)]},
        {"$gt":[server_time("$lease.expires_at"),"$$NOW"]}
    ]}}
}
fn schedule_fence(value: &HomeBackupSchedule, now: &str) -> Result<Document, HomeBackupError> {
    value.validate()?;
    canonical(now)?;
    let lease = value.lease.as_ref().ok_or(HomeBackupError::Conflict)?;
    let mut filter = live(lease, now);
    filter.insert("_id", &value.id);
    filter.insert(
        "revision",
        codec::signed(value.revision).map_err(malformed)?,
    );
    filter.insert("state", "active");
    Ok(filter)
}
fn backup_fence(value: &HomeBackup, now: &str) -> Result<Document, HomeBackupError> {
    value.validate()?;
    let lease = value.lease.as_ref().ok_or(HomeBackupError::Conflict)?;
    let mut filter = live(lease, now);
    filter.insert("_id", &value.id);
    filter.insert(
        "revision",
        codec::signed(value.revision).map_err(malformed)?,
    );
    Ok(filter)
}
async fn commit(session: &mut ClientSession) -> Result<(), HomeBackupError> {
    match commit_transaction(session).await {
        CommitOutcome::Committed => Ok(()),
        CommitOutcome::Retry(_) => Err(HomeBackupError::Conflict),
        CommitOutcome::Unknown(_) => Err(HomeBackupError::CommitUnknown),
        CommitOutcome::Failed(_) => Err(HomeBackupError::Unavailable),
    }
}
async fn abort(session: &mut ClientSession) {
    // Dropping a session also aborts. Await explicit cleanup when possible;
    // never turn a failed transaction into successful publication.
    let _ = session.abort_guarded().await;
}

pub(super) async fn claim(
    store: &MongoHomeBackupStore,
    now: &str,
    holder: &str,
    until: &str,
) -> Result<Option<HomeBackupClaim>, HomeBackupError> {
    let now_seconds = canonical(now)?;
    if canonical(until)? <= now_seconds {
        return Err(HomeBackupError::Conflict);
    }
    let candidate = store
        .schedules
        .find_one(doc! {"state":"active",
        "$expr":{"$and":[
            {"$lte":[server_time("$next_due_at"),"$$NOW"]},
            {"$or":[{"$eq":["$lease",null]},{"$lte":[server_time("$lease.expires_at"),"$$NOW"]}]}
        ]}})
        .sort(doc! {"next_due_at":1,"_id":1})
        .await
        .map_err(failed)?;
    let Some(row) = candidate else {
        return Ok(None);
    };
    let previous = read_schedule(row).map_err(malformed)?;
    let mut schedule = previous.clone();
    schedule.claim(
        now,
        HomeBackupLease {
            holder: holder.into(),
            token: uuid::Uuid::new_v4().to_string(),
            fence: previous.next_fence,
            expires_at: until.into(),
        },
    )?;
    let mut filter = doc! {"_id":&previous.id,"revision":codec::signed(previous.revision).map_err(malformed)?,"state":"active",
    "$expr":{"$and":[
        {"$lte":[server_time("$next_due_at"),"$$NOW"]},
        {"$gt":[server_time(until),"$$NOW"]},
        {"$or":[{"$eq":["$lease",null]},{"$lte":[server_time("$lease.expires_at"),"$$NOW"]}]}
    ]}};
    filter.insert(
        "next_fence",
        codec::signed(previous.next_fence).map_err(malformed)?,
    );
    let mut session = store.client.start_session().await.map_err(failed)?;
    start_transaction(&mut session).await.map_err(failed)?;
    let outcome = async {
        let result = store.schedules.replace_one(filter, schedule_document(&schedule).map_err(malformed)?).session(&mut session).await.map_err(failed)?;
        if result.matched_count != 1 { return Err(HomeBackupError::Conflict); }
        let backup = if let Some(flight) = &schedule.in_flight {
            let row = store.backups.find_one(doc! {"_id":&flight.backup_id}).session(&mut session).await.map_err(failed)?.ok_or(HomeBackupError::Invalid("in-flight backup absent"))?;
            let previous = read_backup(row).map_err(malformed)?;
            if previous.sandbox_id != schedule.id || !matches!(previous.state, HomeBackupState::Requested|HomeBackupState::Creating) { return Err(HomeBackupError::Conflict); }
            if previous
                .lease
                .as_ref()
                .is_some_and(|lease| canonical(&lease.expires_at).is_ok_and(|expires| expires > now_seconds))
            {
                return Err(HomeBackupError::Conflict);
            }
            let mut next = previous.clone();
            next.lease = Some(HomeBackupLease { holder:holder.into(),token:uuid::Uuid::new_v4().to_string(),fence:previous.next_fence,expires_at:until.into() });
            next.next_fence = increment(previous.next_fence)?;
            next.revision = increment(previous.revision)?;
            let result = store.backups.replace_one(doc! {"_id":&next.id,"revision":codec::signed(previous.revision).map_err(malformed)?,"$expr":{"$or":[{"$eq":["$lease",null]},{"$lte":[server_time("$lease.expires_at"),"$$NOW"]}]}}, backup_document(&next).map_err(malformed)?).session(&mut session).await.map_err(failed)?;
            if result.matched_count != 1 { return Err(HomeBackupError::Conflict); }
            Some(next)
        } else { None };
        Ok(HomeBackupClaim { schedule, backup })
    }.await;
    match outcome {
        Ok(claim) => {
            commit(&mut session).await?;
            Ok(Some(claim))
        }
        Err(error) => {
            abort(&mut session).await;
            Err(error)
        }
    }
}

pub(super) async fn plan(
    store: &MongoHomeBackupStore,
    claim: &HomeBackupClaim,
    backup: &HomeBackup,
    now: &str,
) -> Result<HomeBackupClaim, HomeBackupError> {
    backup.validate()?;
    if claim.backup.is_some()
        || claim.schedule.in_flight.is_some()
        || backup.sandbox_id != claim.schedule.id
        || backup.epoch
            != claim
                .schedule
                .lease
                .as_ref()
                .ok_or(HomeBackupError::Conflict)?
                .fence
        || backup.state != HomeBackupState::Requested
        || backup.revision != 1
        || backup.lease.is_some()
        || backup.next_fence != 1
        || backup.source.home_source_id != claim.schedule.home.home_source_id
        || backup.source.home_identity != claim.schedule.home.home_identity
    {
        return Err(HomeBackupError::Conflict);
    }
    let mut schedule = claim.schedule.clone();
    schedule.revision = increment(schedule.revision)?;
    schedule.in_flight = Some(HomeBackupInFlight {
        backup_id: backup.id.clone(),
        phase: HomeBackupPhase::Requested,
        started_at: backup.cut.requested_at.clone(),
    });
    let mut backup = backup.clone();
    let schedule_lease = schedule.lease.as_ref().ok_or(HomeBackupError::Conflict)?;
    backup.lease = Some(HomeBackupLease {
        holder: schedule_lease.holder.clone(),
        token: uuid::Uuid::new_v4().to_string(),
        fence: 1,
        expires_at: schedule_lease.expires_at.clone(),
    });
    backup.next_fence = 2;
    let mut session = store.client.start_session().await.map_err(failed)?;
    start_transaction(&mut session).await.map_err(failed)?;
    let outcome = async {
        let result = store
            .schedules
            .replace_one(
                schedule_fence(&claim.schedule, now)?,
                schedule_document(&schedule).map_err(malformed)?,
            )
            .session(&mut session)
            .await
            .map_err(failed)?;
        if result.matched_count != 1 {
            return Err(HomeBackupError::Conflict);
        }
        store
            .backups
            .insert_one(backup_document(&backup).map_err(malformed)?)
            .session(&mut session)
            .await
            .map_err(failed)?;
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => {
            commit(&mut session).await?;
            Ok(HomeBackupClaim {
                schedule,
                backup: Some(backup),
            })
        }
        Err(error) => {
            abort(&mut session).await;
            Err(error)
        }
    }
}

pub(super) async fn save(
    store: &MongoHomeBackupStore,
    claim: &HomeBackupClaim,
    schedule: &HomeBackupSchedule,
    backup: &HomeBackup,
    now: &str,
) -> Result<HomeBackupClaim, HomeBackupError> {
    let previous = claim.backup.as_ref().ok_or(HomeBackupError::Conflict)?;
    // External effects may only publish to the same immutable capture intent.
    if schedule.id != claim.schedule.id
        || schedule.home != claim.schedule.home
        || schedule.state != HomeBackupScheduleState::Active
        || schedule.next_fence != claim.schedule.next_fence
        || backup.id != previous.id
        || backup.sandbox_id != previous.sandbox_id
        || backup.epoch != previous.epoch
        || backup.source != previous.source
        || backup.custody != previous.custody
        || backup.cut.requested_at != previous.cut.requested_at
        || backup.storage.snapshot_name != previous.storage.snapshot_name
        || backup.storage.snapshot_class != previous.storage.snapshot_class
        || backup.restore_pins != previous.restore_pins
        || backup.next_fence != previous.next_fence
        || !matches!(
            (previous.state, backup.state),
            (
                HomeBackupState::Requested,
                HomeBackupState::Creating | HomeBackupState::Failed
            ) | (
                HomeBackupState::Creating,
                HomeBackupState::Creating | HomeBackupState::Ready | HomeBackupState::Failed
            )
        )
    {
        return Err(HomeBackupError::Conflict);
    }
    let mut schedule = schedule.clone();
    let mut backup = backup.clone();
    let ready_pointer_changed = schedule.last_ready_at != claim.schedule.last_ready_at
        || schedule.last_ready_backup_id != claim.schedule.last_ready_backup_id;
    if ready_pointer_changed && backup.state != HomeBackupState::Ready {
        return Err(HomeBackupError::Conflict);
    }
    schedule.revision = increment(claim.schedule.revision)?;
    backup.revision = increment(previous.revision)?;
    if matches!(
        backup.state,
        HomeBackupState::Ready | HomeBackupState::Failed
    ) {
        if schedule.in_flight.is_some() {
            return Err(HomeBackupError::Conflict);
        }
        schedule.lease = None;
        backup.lease = None;
        if backup.state == HomeBackupState::Ready
            && (schedule.last_ready_backup_id.as_deref() != Some(&backup.id)
                || schedule.last_ready_at != backup.cut.ready_at)
        {
            return Err(HomeBackupError::Conflict);
        }
    } else if schedule.lease != claim.schedule.lease
        || backup.lease != previous.lease
        || schedule.in_flight.as_ref().map(|value| &value.backup_id) != Some(&backup.id)
    {
        return Err(HomeBackupError::Conflict);
    }
    schedule.validate()?;
    backup.validate()?;
    let mut session = store.client.start_session().await.map_err(failed)?;
    start_transaction(&mut session).await.map_err(failed)?;
    let outcome = async {
        let result = store
            .schedules
            .replace_one(
                schedule_fence(&claim.schedule, now)?,
                schedule_document(&schedule).map_err(malformed)?,
            )
            .session(&mut session)
            .await
            .map_err(failed)?;
        if result.matched_count != 1 {
            return Err(HomeBackupError::Conflict);
        }
        if backup.state == HomeBackupState::Ready {
            super::retention::publish(store, &mut backup, &mut session).await?;
        }
        let result = store
            .backups
            .replace_one(
                backup_fence(previous, now)?,
                backup_document(&backup).map_err(malformed)?,
            )
            .session(&mut session)
            .await
            .map_err(failed)?;
        if result.matched_count != 1 {
            return Err(HomeBackupError::Conflict);
        }
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => {
            commit(&mut session).await?;
            Ok(HomeBackupClaim {
                schedule,
                backup: Some(backup),
            })
        }
        Err(error) => {
            abort(&mut session).await;
            Err(error)
        }
    }
}
