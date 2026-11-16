//! Durable account-erasure admission for home backups.
//!
//! This module only fences Mongo records and claims provider effects. The
//! irreversible delete is supplied by the layer-3 deletion port.

use super::*;
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use futures_util::TryStreamExt;
use layer2_core::sandbox::home_backup::*;
use mongodb::{bson::doc, ClientSession};

fn canonical(value: &str) -> Result<(), HomeBackupError> {
    layer2_core::seconds_from_rfc3339(value)
        .map(|_| ())
        .ok_or(HomeBackupError::Invalid("noncanonical timestamp"))
}

fn date(value: &str) -> mongodb::bson::Document {
    doc! {"$dateFromString":{"dateString":value,"onError":null,"onNull":null}}
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
impl HomeBackupErasureStore for MongoHomeBackupStore {
    async fn claim_sandbox_erasure(
        &self,
        sandbox_id: &str,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<Vec<HomeBackup>, HomeBackupError> {
        canonical(now)?;
        canonical(until)?;
        if sandbox_id.is_empty() || sandbox_id.trim() != sandbox_id || holder.is_empty() {
            return Err(HomeBackupError::Invalid("invalid erasure identity"));
        }
        let mut session = self.client.start_session().await.map_err(failed)?;
        start_transaction(&mut session).await.map_err(failed)?;
        let outcome = async {
            let row = self
                .schedules
                .find_one(doc! {"_id":sandbox_id})
                .session(&mut session)
                .await
                .map_err(failed)?
                .ok_or(HomeBackupError::Conflict)?;
            let parent = read_schedule(row).map_err(malformed)?;
            if parent.state == HomeBackupScheduleState::Closed {
                return Err(HomeBackupError::Conflict);
            }
            let mut fenced = parent.clone();
            if parent.state != HomeBackupScheduleState::Erasing {
                fenced.state = HomeBackupScheduleState::Erasing;
                fenced.lease = None;
                fenced.in_flight = None;
                fenced.revision = increment(parent.revision)?;
                fenced.next_fence = increment(parent.next_fence)?;
                let result = self
                    .schedules
                    .replace_one(
                        doc! {
                            "_id":&parent.id,
                            "revision":codec::signed(parent.revision).map_err(malformed)?,
                            "next_fence":codec::signed(parent.next_fence).map_err(malformed)?
                        },
                        schedule_document(&fenced).map_err(malformed)?,
                    )
                    .session(&mut session)
                    .await
                    .map_err(failed)?;
                if result.matched_count != 1 {
                    return Err(HomeBackupError::Conflict);
                }
            } else if parent.lease.is_some() || parent.in_flight.is_some() {
                return Err(HomeBackupError::Conflict);
            }

            let mut cursor = self
                .backups
                .find(doc! {"sandbox_id":sandbox_id,"state":{"$ne":"deleted"}})
                .session(&mut session)
                .await
                .map_err(failed)?;
            let mut claimed = Vec::new();
            while let Some(row) = cursor.try_next().await.map_err(failed)? {
                let previous = read_backup(row).map_err(malformed)?;
                if previous.state == HomeBackupState::Deleted {
                    continue;
                }
                let next = previous.claim_deletion(
                    HomeBackupDeletionMode::Erasure,
                    now,
                    HomeBackupLease {
                        holder: holder.into(),
                        token: uuid::Uuid::new_v4().to_string(),
                        fence: previous.next_fence,
                        expires_at: until.into(),
                    },
                )?;
                let result = self
                    .backups
                    .replace_one(
                        doc! {
                            "_id":&previous.id,
                            "sandbox_id":sandbox_id,
                            "revision":codec::signed(previous.revision).map_err(malformed)?,
                            "next_fence":codec::signed(previous.next_fence).map_err(malformed)?,
                            "restore_pins":0,
                            "$expr":{"$or":[
                                {"$eq":["$lease",null]},
                                {"$lte":[date("$lease.expires_at"),date(now)]},
                                {"$lte":[date("$lease.expires_at"),"$$NOW"]}
                            ]}
                        },
                        backup_document(&next).map_err(malformed)?,
                    )
                    .session(&mut session)
                    .await
                    .map_err(failed)?;
                if result.matched_count != 1 {
                    return Err(HomeBackupError::Conflict);
                }
                claimed.push(next);
            }
            Ok(claimed)
        }
        .await;
        match outcome {
            Ok(claimed) => {
                commit(&mut session).await?;
                Ok(claimed)
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                Err(error)
            }
        }
    }
}
