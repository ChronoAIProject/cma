//! Bounded migration of unowned legacy reservations into durable cleanup work.

use futures_util::TryStreamExt;
use layer2_core::sandbox::{SandboxStoreError, WarmSandboxState};
use mongodb::bson::doc;

use super::{document, read, unavailable};
use crate::store::sandbox_mongo::MongoSandboxStore;

impl MongoSandboxStore {
    pub(crate) async fn retire_legacy_warmups(&self, at: &str) -> Result<(), SandboxStoreError> {
        let mut cursor = self
            .warm_sandboxes
            .find(doc! {"state":{"$in":["preparing","installing","ready"]}})
            .sort(doc! {"_id":1})
            .limit(128)
            .await
            .map_err(unavailable)?;
        while let Some(old) = cursor.try_next().await.map_err(unavailable)? {
            let mut record = read(&old)?;
            record.state = WarmSandboxState::Retiring;
            record.revision = record
                .revision
                .checked_add(1)
                .ok_or(SandboxStoreError::Conflict)?;
            record.updated_at = at.into();
            let mut update = document(&record)?;
            update.remove("_id");
            update.insert("check_after", at);
            // This write conflicts with an old transaction claiming this Ready
            // row. A worker holding an earlier revision can no longer publish.
            // Preserve its lease fields: deletion must wait for it to exit/expire.
            self.warm_sandboxes.update_one(
                doc! {"_id": &record.id.0,"revision":old.get("revision").cloned(),"state":old.get("state").cloned()},
                doc! {"$set":update},
            ).await.map_err(unavailable)?;
        }
        Ok(())
    }
}
