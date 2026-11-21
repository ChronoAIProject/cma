//! Reviewed catalog metadata; preparation is never part of service startup.
use super::{BACKUPS_COLLECTION, SCHEDULES_COLLECTION};
use crate::store::schema::index_contract_v1 as metadata;
use mongodb::{
    bson::{doc, Document},
    Database,
};

pub(in crate::store) const COLLECTIONS: [&str; 2] = [SCHEDULES_COLLECTION, BACKUPS_COLLECTION];

pub(in crate::store) fn indexes(collection: &str) -> Vec<Document> {
    match collection {
        SCHEDULES_COLLECTION => vec![doc! {
            "name":"state_1_next_due_at_1__id_1",
            "key":{"state":1,"next_due_at":1,"_id":1},
        }],
        BACKUPS_COLLECTION => vec![
            doc! {"name":"sandbox_id_1_epoch_1","key":{"sandbox_id":1,"epoch":1},"unique":true},
            doc! {"name":"sandbox_id_1_state_1_cut.ready_at_-1","key":{"sandbox_id":1,"state":1,"cut.ready_at":-1}},
            doc! {"name":"state_1_retention.expires_at_1__id_1","key":{"state":1,"retention.expires_at":1,"_id":1}},
        ],
        _ => Vec::new(),
    }
}

pub(super) async fn require_prepared(database: &Database) -> Result<(), layer2_core::StoreError> {
    let inspected = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        for collection in COLLECTIONS {
            let existing = metadata::inspect_collection(database, collection)
                .await
                .map_err(<&str>::from)?
                .ok_or("home backup schema absent; explicit preparation required")?;
            let desired = indexes(collection);
            if existing.len() != desired.len() + 1 {
                return Err("home backup index inventory differs from reviewed contract");
            }
            for index in desired {
                if !metadata::missing(&existing, &index)?.is_empty() {
                    return Err("home backup index absent; explicit preparation required");
                }
            }
        }
        Ok(())
    })
    .await
    .unwrap_or(Err("home backup schema inspection timed out"));
    inspected.map_err(|reason| layer2_core::StoreError::Unavailable(reason.into()))
}
