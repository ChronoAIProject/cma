use super::*;

impl ReportingStore {
    pub(crate) async fn initialize_allocation_buckets(&self, enabled: bool) -> Result<()> {
        if !enabled {
            return Ok(());
        }
        super::super::schema::initialize_collections(&self.db, &[COLLECTION])
            .await
            .map_err(unavailable)?;
        self.index(COLLECTION, doc! {"sandbox_id":1,"day":1}, true, false)
            .await?;
        self.index(COLLECTION, doc! {"expires_at":1}, false, true)
            .await?;
        for lane in [worker::Lane::Live, worker::Lane::Backfill] {
            self.collection(CONTROL)
                .update_one(
                    doc! {"_id":lane.key()},
                    doc! {
                        "$setOnInsert": {"document_schema_version":1_i64,"generation":0_i64,
                            "lease_until":mongodb::bson::DateTime::from_millis(0),
                            "next_at":mongodb::bson::DateTime::from_millis(0),"holder":""}
                    },
                )
                .upsert(true)
                .await
                .map_err(unavailable)?;
        }
        Ok(())
    }
}
