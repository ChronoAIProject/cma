//! Discovery metadata never mutates the immutable manifest's bytes or version.
use super::{failure, MongoRecords, RecordError, RecordHead, INTERACTIONS};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::{Acknowledgment, CollectionOptions, ReadConcern, WriteConcern},
};

impl MongoRecords {
    pub(crate) async fn finish_admission_index(
        &self,
        id: &str,
        owner: &str,
        version: u64,
        deployment: &str,
    ) -> Result<(), RecordError> {
        if version != 1 || deployment.len() != 64 {
            return Err(RecordError::Rejected(
                "invalid admission index fence".into(),
            ));
        }
        let records = self.db.collection_with_options::<Document>(
            INTERACTIONS,
            CollectionOptions::builder()
                .read_concern(ReadConcern::majority())
                .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
                .build(),
        );
        let filter =
            doc! {"_id":id,"owner":owner,"version":1_i64,"$expr":{"$gt":["$expires_at","$$NOW"]}};
        let mut indexed = filter.clone();
        indexed.insert("replica.kind", "admission");
        indexed.insert("replica.deployment", deployment);
        let changed = records
            .update_one(indexed, doc! {"$set":{"replica.active":false}})
            .await
            .map_err(failure)?;
        if changed.matched_count == 1 {
            return Ok(());
        }
        // Explicit legacy repair may queue a pre-mode root. No projection means
        // there is nothing to clear; a different indexed kind/deployment fails.
        let mut legacy = filter;
        legacy.insert("replica", doc! {"$exists":false});
        if records.find_one(legacy).await.map_err(failure)?.is_some() {
            Ok(())
        } else {
            Err(RecordError::Conflict {
                expected: 1,
                actual: None,
            })
        }
    }
    pub(crate) async fn list_admission_roots(
        &self,
        deployment: &str,
        owner: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RecordHead>, RecordError> {
        if !(1..=32).contains(&limit) || deployment.len() != 64 {
            return Err(RecordError::Rejected(
                "invalid admission discovery bounds".into(),
            ));
        }
        let mut filter = doc! {"replica":{"$exists":true},"replica.kind":"admission","replica.deployment":deployment,"replica.active":true,
        "$expr":{"$gt":["$expires_at","$$NOW"]}};
        if let Some(owner) = owner {
            filter.insert("owner", owner);
        }
        if let Some(after) = after {
            filter.insert("_id", doc! {"$gt":after});
        }
        let records = self.db.collection_with_options::<Document>(
            INTERACTIONS,
            CollectionOptions::builder()
                .read_concern(ReadConcern::majority())
                .build(),
        );
        let rows: Vec<Document> = records
            .find(filter)
            .projection(doc! {"_id":1,"owner":1,"version":1})
            .sort(doc! {"_id":1})
            .limit(limit as i64)
            .await
            .map_err(failure)?
            .try_collect()
            .await
            .map_err(failure)?;
        rows.into_iter()
            .map(|row| {
                let invalid =
                    || RecordError::Malformed("invalid admission discovery record".into());
                if row.get_i64("version").map_err(|_| invalid())? != 1 {
                    return Err(invalid());
                }
                Ok(RecordHead {
                    record_id: row.get_str("_id").map_err(|_| invalid())?.into(),
                    owner: row.get_str("owner").map_err(|_| invalid())?.into(),
                    version: 1,
                })
            })
            .collect()
    }
}
