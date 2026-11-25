//! Majority-backed authority for independent home backup capture.
use codec::{backup_document, read_backup, read_schedule, schedule_document};
use futures_util::TryStreamExt;
use layer2_core::sandbox::home_backup::*;
use mongodb::{
    bson::{doc, Document},
    Client, Collection,
};
mod capture;
mod codec;
mod discovery;
pub(crate) mod evidence_export;
mod expiry;
mod retention;
mod retirement;
pub(in crate::store) mod schema_v1;
#[cfg(test)]
mod tests;

const SCHEMA_VERSION: i32 = 1;
pub const SCHEDULES_COLLECTION: &str = "home_backup_schedules";
pub const BACKUPS_COLLECTION: &str = "home_backups";

#[derive(Clone)]
pub(crate) struct MongoHomeBackupStore {
    client: Client,
    schedules: Collection<Document>,
    backups: Collection<Document>,
}
impl MongoHomeBackupStore {
    pub(crate) async fn open(
        client: &Client,
        database: &str,
    ) -> Result<Self, layer2_core::StoreError> {
        let db = client.database(database);
        schema_v1::require_prepared(&db).await?;
        Ok(Self {
            client: client.clone(),
            schedules: super::durability::majority_collection(&db, SCHEDULES_COLLECTION),
            backups: super::durability::majority_collection(&db, BACKUPS_COLLECTION),
        })
    }
}

#[async_trait::async_trait]
impl HomeBackupStore for MongoHomeBackupStore {
    async fn get_schedule(&self, id: &str) -> Result<Option<HomeBackupSchedule>, HomeBackupError> {
        self.schedules
            .find_one(doc! {"_id":id})
            .await
            .map_err(failed)?
            .map(read_schedule)
            .transpose()
            .map_err(malformed)
    }
    async fn get_backup(&self, id: &str) -> Result<Option<HomeBackup>, HomeBackupError> {
        self.backups
            .find_one(doc! {"_id":id})
            .await
            .map_err(failed)?
            .map(read_backup)
            .transpose()
            .map_err(malformed)
    }
    async fn ensure_schedule(&self, value: &HomeBackupSchedule) -> Result<(), HomeBackupError> {
        if value.lease.is_some()
            || value.in_flight.is_some()
            || value.state != HomeBackupScheduleState::Active
            || value.revision != 1
            || value.next_fence != 1
        {
            return Err(HomeBackupError::Conflict);
        }
        let row = schedule_document(value).map_err(malformed)?;
        self.schedules
            .update_one(doc! {"_id":&value.id}, doc! {"$setOnInsert":row})
            .upsert(true)
            .await
            .map_err(failed)?;
        let stored = self
            .get_schedule(&value.id)
            .await?
            .ok_or(HomeBackupError::Conflict)?;
        if stored.home != value.home {
            return Err(HomeBackupError::Conflict);
        }
        Ok(())
    }
    async fn claim_due(
        &self,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<Option<HomeBackupClaim>, HomeBackupError> {
        capture::claim(self, now, holder, until).await
    }
    async fn plan_capture(
        &self,
        claim: &HomeBackupClaim,
        backup: &HomeBackup,
        now: &str,
    ) -> Result<HomeBackupClaim, HomeBackupError> {
        capture::plan(self, claim, backup, now).await
    }
    async fn save_capture(
        &self,
        claim: &HomeBackupClaim,
        schedule: &HomeBackupSchedule,
        backup: &HomeBackup,
        now: &str,
    ) -> Result<HomeBackupClaim, HomeBackupError> {
        capture::save(self, claim, schedule, backup, now).await
    }
}
fn failed(error: mongodb::error::Error) -> HomeBackupError {
    if super::sandbox_mongo::is_transient(&error) {
        HomeBackupError::Conflict
    } else {
        HomeBackupError::Unavailable
    }
}
fn malformed(_: layer2_core::StoreError) -> HomeBackupError {
    HomeBackupError::Invalid("invalid stored home backup")
}
