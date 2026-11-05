use super::*;
use layer2_core::sandbox_metrics::Aggregate;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LiveReading {
    pub owner: String,
    pub observed_at: u64,
    pub metrics: [Aggregate; 3],
}
impl ReportingStore {
    pub(crate) async fn save_live(&self, readings: &[(String, LiveReading)]) -> Result<()> {
        let mut updates = Vec::new();
        for (id, reading) in readings {
            let key = format!("{id}:live");
            let d = doc! {"_id":&key,"document_schema_version":1_i64,"sandbox_id":id,"day":-1_i64,"chunk":0_i64,
            "expires_at":expires(reading.observed_at+RETENTION_SECONDS),"record":encode(reading)?};
            updates.push(doc! {"q":{"_id":key},"u":[{"$replaceWith":{"$cond":[
                {"$or":[{"$eq":[{"$type":"$record"},"missing"]},{"$lte":["$record.observed_at",reading.observed_at as i64]}]},
                {"$literal":d},"$$ROOT"]}}],"upsert":true,"multi":false});
        }
        if !updates.is_empty() {
            self.bulk_updates(COLLECTION, updates).await?;
        }
        Ok(())
    }
    pub(crate) async fn live_readings(
        &self,
        ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, LiveReading>> {
        let mut session = self.snapshot().await?;
        self.live_readings_at(ids, &mut session).await
    }
    pub(crate) async fn live_readings_at(
        &self,
        ids: &[String],
        session: &mut mongodb::ClientSession,
    ) -> Result<std::collections::BTreeMap<String, LiveReading>> {
        let mut cursor = self
            .collection(COLLECTION)
            .find(doc! {"sandbox_id":{"$in":ids},"day":-1_i64})
            .max_time(DB_DEADLINE)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let mut out = std::collections::BTreeMap::new();
        while let Some(d) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            out.insert(
                d.get_str("sandbox_id").map_err(unavailable)?.into(),
                decode(&d)?,
            );
        }
        Ok(out)
    }
}
