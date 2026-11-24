//! Destructive effects are admitted durably and settled only with exact evidence.
use super::*;
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::ClientSession;

fn canonical(value: &str) -> Result<(), HomeBackupError> {
    layer2_core::seconds_from_rfc3339(value)
        .map(|_| ())
        .ok_or(HomeBackupError::Invalid("noncanonical timestamp"))
}

fn date(value: &str) -> Document {
    doc! {"$dateFromString":{"dateString":value,"onError":null,"onNull":null}}
}

fn available(now: &str, until: &str) -> Document {
    doc! {"$and":[
        {"$gt":[date(until),"$$NOW"]},
        {"$gt":[date(until),date(now)]},
        {"$or":[{"$eq":["$lease",null]},{"$and":[
            {"$lte":[date("$lease.expires_at"),"$$NOW"]},
            {"$lte":[date("$lease.expires_at"),date(now)]}
        ]}]}
    ]}
}

fn claim_filter(now: &str, until: &str) -> Document {
    doc! {"restore_pins":0,"$expr":available(now,until)}
}

fn expiry_filter(now: &str, until: &str) -> Document {
    let mut filter = claim_filter(now, until);
    filter.insert(
        "$or",
        mongodb::bson::serialize_to_bson(&[
            doc! {"state":"expiring"},
            doc! {"state":{"$in":["ready","failed"]},"$expr":{"$and":[
                {"$lte":[date("$retention.expires_at"),"$$NOW"]},
                {"$lte":[date("$retention.expires_at"),date(now)]}
            ]}},
        ])
        .expect("document array serializes"),
    );
    filter
}

fn claim_next(
    previous: &HomeBackup,
    mode: HomeBackupDeletionMode,
    now: &str,
    holder: &str,
    until: &str,
) -> Result<HomeBackup, HomeBackupError> {
    previous.claim_deletion(
        mode,
        now,
        HomeBackupLease {
            holder: holder.into(),
            token: uuid::Uuid::new_v4().to_string(),
            fence: previous.next_fence,
            expires_at: until.into(),
        },
    )
}

fn exact_claim(filter: &mut Document, value: &HomeBackup) -> Result<(), HomeBackupError> {
    filter.insert("_id", &value.id);
    filter.insert(
        "revision",
        codec::signed(value.revision).map_err(malformed)?,
    );
    filter.insert(
        "next_fence",
        codec::signed(value.next_fence).map_err(malformed)?,
    );
    Ok(())
}

async fn commit(session: &mut ClientSession) -> Result<(), HomeBackupError> {
    match commit_transaction(session).await {
        CommitOutcome::Committed => Ok(()),
        CommitOutcome::Retry(_) => Err(HomeBackupError::Conflict),
        CommitOutcome::Unknown(_) => Err(HomeBackupError::CommitUnknown),
        CommitOutcome::Failed(_) => Err(HomeBackupError::Unavailable),
    }
}

#[async_trait::async_trait]
impl HomeBackupDeletionStore for MongoHomeBackupStore {
    async fn claim_expiry(
        &self,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<Option<HomeBackup>, HomeBackupError> {
        canonical(now)?;
        canonical(until)?;
        let mut filter = expiry_filter(now, until);
        let Some(row) = self
            .backups
            .find_one(filter.clone())
            .sort(doc! {"retention.expires_at":1,"_id":1})
            .await
            .map_err(failed)?
        else {
            return Ok(None);
        };
        let previous = read_backup(row).map_err(malformed)?;
        let next = claim_next(
            &previous,
            HomeBackupDeletionMode::Expiry,
            now,
            holder,
            until,
        )?;
        exact_claim(&mut filter, &previous)?;
        let result = self
            .backups
            .replace_one(filter, backup_document(&next).map_err(malformed)?)
            .await
            .map_err(failed)?;
        if result.matched_count != 1 {
            return Err(HomeBackupError::Conflict);
        }
        Ok(Some(next))
    }

    async fn claim_expiring(
        &self,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<Option<HomeBackup>, HomeBackupError> {
        canonical(now)?;
        canonical(until)?;
        let mut filter = claim_filter(now, until);
        filter.insert("state", "expiring");
        let Some(row) = self
            .backups
            .find_one(filter.clone())
            .sort(doc! {"retention.expires_at":1,"_id":1})
            .await
            .map_err(failed)?
        else {
            return Ok(None);
        };
        let previous = read_backup(row).map_err(malformed)?;
        let next = claim_next(
            &previous,
            HomeBackupDeletionMode::Expiry,
            now,
            holder,
            until,
        )?;
        exact_claim(&mut filter, &previous)?;
        let result = self
            .backups
            .replace_one(filter, backup_document(&next).map_err(malformed)?)
            .await
            .map_err(failed)?;
        if result.matched_count != 1 {
            return Err(HomeBackupError::Conflict);
        }
        Ok(Some(next))
    }

    async fn claim_erasure(
        &self,
        backup_id: &str,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<HomeBackup, HomeBackupError> {
        canonical(now)?;
        canonical(until)?;
        let mut session = self.client.start_session().await.map_err(failed)?;
        start_transaction(&mut session).await.map_err(failed)?;
        let outcome = async {
            let row = self
                .backups
                .find_one(doc! {"_id":backup_id})
                .session(&mut session)
                .await
                .map_err(failed)?
                .ok_or(HomeBackupError::Conflict)?;
            let previous = read_backup(row).map_err(malformed)?;
            let row = self
                .schedules
                .find_one(doc! {"_id":&previous.sandbox_id})
                .session(&mut session)
                .await
                .map_err(failed)?
                .ok_or(HomeBackupError::Conflict)?;
            let parent = read_schedule(row).map_err(malformed)?;
            if parent.state != HomeBackupScheduleState::Erasing
                || parent.in_flight.is_some()
                || parent.lease.is_some()
            {
                return Err(HomeBackupError::Conflict);
            }
            let next = claim_next(
                &previous,
                HomeBackupDeletionMode::Erasure,
                now,
                holder,
                until,
            )?;
            let mut filter = claim_filter(now, until);
            exact_claim(&mut filter, &previous)?;
            filter.insert(
                "state",
                mongodb::bson::serialize_to_bson(&previous.state)
                    .map_err(|_| HomeBackupError::Invalid("invalid state"))?,
            );
            let result = self
                .backups
                .replace_one(filter, backup_document(&next).map_err(malformed)?)
                .session(&mut session)
                .await
                .map_err(failed)?;
            if result.matched_count != 1 {
                return Err(HomeBackupError::Conflict);
            }
            // Write the schedule too, so a concurrent state/fence change cannot
            // pass using a transaction's older snapshot of the admission row.
            let mut updated = parent.clone();
            updated.revision = increment(parent.revision)?;
            let result = self
                .schedules
                .replace_one(
                    doc! {
                        "_id":&parent.id,"state":"erasing",
                        "revision":codec::signed(parent.revision).map_err(malformed)?,
                        "next_fence":codec::signed(parent.next_fence).map_err(malformed)?
                    },
                    schedule_document(&updated).map_err(malformed)?,
                )
                .session(&mut session)
                .await
                .map_err(failed)?;
            if result.matched_count != 1 {
                return Err(HomeBackupError::Conflict);
            }
            Ok(next)
        }
        .await;
        match outcome {
            Ok(next) => {
                commit(&mut session).await?;
                Ok(next)
            }
            Err(error) => {
                let _ = session.abort_guarded().await;
                Err(error)
            }
        }
    }

    async fn save_deletion(
        &self,
        claimed: &HomeBackup,
        observation: &HomeBackupDeletionObservation,
        now: &str,
    ) -> Result<HomeBackup, HomeBackupError> {
        let next = claimed.complete_deletion(observation, now)?;
        let lease = claimed.lease.as_ref().ok_or(HomeBackupError::Conflict)?;
        let mut filter = doc! {
            "state":"expiring","restore_pins":0,
            "lease.token":&lease.token,"lease.fence":codec::signed(lease.fence).map_err(malformed)?,
            "$expr":{"$and":[
                {"$gt":[date("$lease.expires_at"),"$$NOW"]},
                {"$gt":[date("$lease.expires_at"),date(now)]},
                {"$lte":[date(&observation.observed_at),"$$NOW"]}
            ]}
        };
        exact_claim(&mut filter, claimed)?;
        let mut session = self.client.start_session().await.map_err(failed)?;
        start_transaction(&mut session).await.map_err(failed)?;
        let outcome = self
            .backups
            .replace_one(filter, backup_document(&next).map_err(malformed)?)
            .session(&mut session)
            .await;
        match outcome {
            Ok(result) if result.matched_count == 1 => {
                commit(&mut session).await?;
                Ok(next)
            }
            result => {
                let _ = session.abort_guarded().await;
                Err(result
                    .err()
                    .map(failed)
                    .unwrap_or(HomeBackupError::Conflict))
            }
        }
    }
}
