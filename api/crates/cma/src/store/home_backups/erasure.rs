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
    async fn fence_sandbox_erasure(
        &self,
        sandbox_id: &str,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<(), HomeBackupError> {
        canonical(now)?;
        canonical(until)?;
        if layer2_core::seconds_from_rfc3339(until) <= layer2_core::seconds_from_rfc3339(now) {
            return Err(HomeBackupError::Conflict);
        }
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
            if parent.state == HomeBackupScheduleState::Erasing {
                if parent.lease.is_some() || parent.in_flight.is_some() {
                    return Err(HomeBackupError::Conflict);
                }
                return Ok(());
            }
            let mut fenced = parent.clone();
            {
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
            }
            Ok(())
        }
        .await;
        match outcome {
            Ok(()) => {
                commit(&mut session).await?;
                Ok(())
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                Err(error)
            }
        }
    }

    async fn list_sandbox_backups(&self, sandbox_id: &str) -> Result<Vec<String>, HomeBackupError> {
        let mut cursor = self
            .backups
            .find(doc! {"sandbox_id":sandbox_id,"state":{"$ne":"deleted"}})
            .await
            .map_err(failed)?;
        let mut ids = Vec::new();
        while let Some(row) = cursor.try_next().await.map_err(failed)? {
            ids.push(read_backup(row).map_err(malformed)?.id);
        }
        Ok(ids)
    }
}
