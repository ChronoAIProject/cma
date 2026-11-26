//! Resumable read-model jobs. Progress never changes the allocation ledger.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Progress {
    pub after: Option<String>,
    pub from: u64,
    pub to: u64,
}
impl ReportingStore {
    pub(crate) async fn progress(&self, key: &str) -> Result<Progress> {
        self.collection(CONTROL)
            .find_one(doc! {"_id":key})
            .await
            .map_err(unavailable)?
            .filter(|d| d.contains_key("record"))
            .map(|d| decode(&d))
            .transpose()
            .map(|v| v.unwrap_or_default())
    }
    pub(crate) async fn save_progress(
        &self,
        key: &str,
        holder: &str,
        value: &Progress,
    ) -> Result<()> {
        let changed = self
            .collection(CONTROL)
            .update_one(
                cma_am_reporting::control_fence(key, holder),
                doc! {"$set":{"record":encode(value)?}},
            )
            .await
            .map_err(unavailable)?
            .matched_count;
        if changed != 1 {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }
    pub(crate) async fn first_binding_at(&self) -> Result<Option<u64>> {
        self.collection(super::super::sandbox_metric_bindings::COLLECTION)
            .find_one(doc! {})
            .sort(doc! {"valid_from":1})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
            .map(|d| {
                d.get_i64("valid_from")
                    .map(|v| v as u64)
                    .map_err(unavailable)
            })
            .transpose()
    }
}
