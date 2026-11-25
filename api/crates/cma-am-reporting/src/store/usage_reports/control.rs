//! Scheduling tokens belong to one attempt, never to a long-lived process.
use super::{now, CONTROL};
use crate::{pool::DB_DEADLINE, transaction_scope as tx, unavailable, ReportingStore};
use layer2_core::sandbox_metrics::{MetricsError, Result};
use mongodb::bson::{doc, Document};

impl ReportingStore {
    /// Acquire an expired slot with a fresh holder. This is not a renewal API:
    /// repeated calls with a live holder cannot admit overlapping work.
    pub async fn lease(&self, key: &str, holder: &str, seconds: u64) -> Result<bool> {
        let seconds = i64::try_from(seconds).map_err(|_| MetricsError::Unavailable)?;
        if key.is_empty()
            || holder.is_empty()
            || seconds <= 0
            || (now() as i64).checked_add(seconds).is_none()
        {
            return Err(MetricsError::Unavailable);
        }
        let result = self
            .collection(CONTROL)
            .find_one_and_update(
                doc! {"_id":key,"$expr":{"$lte":["$lease_until",server_seconds()]}},
                vec![doc! {"$set":{
                    "holder":{"$literal":holder},
                    "lease_until":{"$add":[server_seconds(),seconds]},
                }}],
            )
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        Ok(result.is_some())
    }

    pub async fn release(&self, key: &str, holder: &str) -> Result<()> {
        let result = self
            .collection(CONTROL)
            .update_one(fence(key, holder), doc! {"$set":{"lease_until":0_i64}})
            .await
            .map_err(unavailable)?;
        if result.matched_count != 1 {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }

    /// A real write in the mapping transaction makes takeover conflict with
    /// every earlier observation, including an unchanged assignment. Keep this
    /// internal counter separate from the public mapping revision.
    pub async fn fence_control_transaction(
        &self,
        key: &str,
        holder: &str,
        session: &mut mongodb::ClientSession,
    ) -> tx::Result<()> {
        let mut filter = fence(key, holder);
        filter.insert(
            "$or",
            mongodb::bson::serialize_to_bson(&[
                doc! {"control_write_fence":{"$exists":false}},
                doc! {"control_write_fence":{"$type":"long","$gte":0_i64,"$lt":i64::MAX}},
            ])
            .map_err(unavailable)?,
        );
        let changed = self
            .collection(CONTROL)
            .update_one(filter, doc! {"$inc":{"control_write_fence":1_i64}})
            .session(session)
            .await
            .map_err(tx::mongo)?
            .matched_count;
        if changed != 1 {
            return Err(MetricsError::Unavailable.into());
        }
        Ok(())
    }
}

pub fn fence(key: &str, holder: &str) -> Document {
    doc! {"_id":key,"holder":holder,
    "$expr":{"$gt":["$lease_until",server_seconds()]}}
}

pub(super) fn server_seconds() -> Document {
    doc! {"$toLong":{"$divide":[{"$toLong":"$$NOW"},1000_i64]}}
}
