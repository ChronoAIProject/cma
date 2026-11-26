//! Server-time leases and transactional publication of data plus resume progress.
use super::worker::{Lane, Progress};
use super::*;
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use mongodb::options::{ReturnDocument, WriteConcern};

pub(super) struct Lease {
    pub lane: Lane,
    pub holder: String,
    pub generation: i64,
    pub at: u64,
    pub progress: Option<Progress>,
}

impl Lease {
    fn fence(&self) -> Document {
        doc! {"_id":self.lane.key(),"holder":&self.holder,"generation":self.generation,
        "$expr":{"$gt":["$lease_until","$$NOW"]}}
    }
}

impl ReportingStore {
    pub(super) async fn claim_allocation_buckets(&self, lane: Lane) -> Result<Option<Lease>> {
        let holder = uuid::Uuid::new_v4().to_string();
        let row = self
            .collection(CONTROL)
            .find_one_and_update(
                doc! {"_id":lane.key(),"document_schema_version":1_i64,
                "$expr":{"$and":[{"$lte":["$lease_until","$$NOW"]},
                    {"$lte":["$next_at","$$NOW"]}]}},
                vec![doc! {"$set":{"holder":{"$literal":&holder},
                "generation":{"$add":["$generation",1_i64]},
                "claimed_at":"$$NOW","lease_until":{"$add":["$$NOW",60_000_i64]}}}],
            )
            .return_document(ReturnDocument::After)
            .write_concern(WriteConcern::majority())
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let millis = row
            .get_datetime("claimed_at")
            .map_err(unavailable)?
            .timestamp_millis();
        let at = u64::try_from(millis).map_err(unavailable)? / 1000;
        let progress = row
            .contains_key("record")
            .then(|| decode(&row))
            .transpose()?;
        let generation = row.get_i64("generation").map_err(unavailable)?;
        if generation < 1 {
            return Err(MetricsError::Unavailable);
        }
        Ok(Some(Lease {
            lane,
            holder,
            generation,
            at,
            progress,
        }))
    }

    pub(super) async fn publish_allocation_buckets(
        &self,
        lease: &Lease,
        buckets: &[DayBucket],
        progress: &Progress,
    ) -> Result<()> {
        if buckets.len() > worker::PAGE_SIZE as usize {
            return Err(MetricsError::Busy);
        }
        let documents = buckets
            .iter()
            .map(DayBucket::document)
            .collect::<Result<Vec<_>>>()?;
        let bytes = documents.iter().try_fold(0, |sum, document| {
            mongodb::bson::serialize_to_vec(document)
                .map(|v| sum + v.len())
                .map_err(unavailable)
        })?;
        if bytes > 4 * 1024 * 1024 {
            return Err(MetricsError::Busy);
        }
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        // Write the lease row before data: takeover conflicts with this exact
        // transaction, including when the old holder resumes after a long pause.
        let changed = self
            .collection(CONTROL)
            .update_one(lease.fence(), doc! {"$inc":{"publication_attempt":1_i64}})
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .matched_count;
        if changed != 1 {
            return Err(MetricsError::Unavailable);
        }
        for (bucket, mut document) in buckets.iter().zip(documents) {
            if let Some(existing) = self
                .collection(COLLECTION)
                .find_one(doc! {"_id":bucket.key()})
                .session(&mut session)
                .await
                .map_err(unavailable)?
            {
                let old = DayBucket::read(&existing)?;
                if !old.may_replace(bucket, &existing, &document)? {
                    continue;
                }
            }
            document.insert("publication_lane", lease.lane.key());
            document.insert("publication_generation", lease.generation);
            self.collection(COLLECTION)
                .replace_one(doc! {"_id":bucket.key()}, document)
                .upsert(true)
                .session(&mut session)
                .await
                .map_err(unavailable)?;
        }
        // Recheck server time after all writes. No partial data, checkpoint,
        // or released lease can become visible if any publication step fails.
        let changed = self.collection(CONTROL).update_one(lease.fence(), vec![doc! {
            "$set":{"record":{"$literal":encode(progress)?},"lease_until":"$$NOW",
                "next_at":{"$add":["$$NOW",if progress.pass_complete {60_000_i64} else {1000_i64}]}}
        }]).session(&mut session).await.map_err(unavailable)?.matched_count;
        if changed != 1 {
            return Err(MetricsError::Unavailable);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(()),
            CommitOutcome::Retry(_) | CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                // An uncertain commit is reconciled by the next fresh claim:
                // its checkpoint and buckets either both advanced or neither did.
                Err(MetricsError::Unavailable)
            }
        }
    }
}
