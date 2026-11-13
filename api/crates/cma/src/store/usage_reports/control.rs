//! Scheduling tokens belong to one attempt, never to a long-lived process.
use super::*;

impl ReportingStore {
    /// Acquire an expired slot with a fresh holder. This is not a renewal API:
    /// repeated calls with a live holder cannot admit overlapping work.
    pub(crate) async fn lease(&self, key: &str, holder: &str, seconds: u64) -> Result<bool> {
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

    pub(crate) async fn release(&self, key: &str, holder: &str) -> Result<()> {
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
}

pub(super) fn fence(key: &str, holder: &str) -> Document {
    doc! {"_id":key,"holder":holder,
    "$expr":{"$gt":["$lease_until",server_seconds()]}}
}

fn server_seconds() -> Document {
    doc! {"$toLong":{"$divide":[{"$toLong":"$$NOW"},1000_i64]}}
}
