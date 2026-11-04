//! Short-lived physical proof continuity, separate from customer assignments.
use super::*;
use layer2_core::sandbox_metrics::{Binding, Infrastructure};
use std::collections::BTreeSet;
impl ReportingStore {
    pub(crate) async fn diagnostic_bindings(
        &self,
        candidates: Vec<Binding>,
    ) -> Result<Vec<Binding>> {
        use futures_util::TryStreamExt;
        let keys: Vec<_> = candidates.iter().map(Binding::physical_key).collect();
        let mut cursor = self
            .collection(super::super::sandbox_metric_bindings::COLLECTION)
            .find(doc! {"active_physical":{"$in":keys}})
            .projection(doc! {"active_physical":1})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        let mut assigned = BTreeSet::new();
        while let Some(d) = cursor.try_next().await.map_err(unavailable)? {
            assigned.insert(
                d.get_str("active_physical")
                    .map_err(unavailable)?
                    .to_owned(),
            );
        }
        let mut result = Vec::new();
        for mut b in candidates {
            if assigned.contains(&b.physical_key()) {
                continue;
            }
            let key = format!("diagnostic:{}", b.physical_key());
            if let Some(d) = self
                .collection(CONTROL)
                .find_one(doc! {"_id":&key})
                .max_time(DB_DEADLINE)
                .await
                .map_err(unavailable)?
            {
                let old: Binding = decode(&d)?;
                let mut normalized = old.clone();
                normalized.valid_from = b.valid_from;
                normalized.observed_at = b.observed_at;
                if normalized == b && b.observed_at.saturating_sub(old.observed_at) <= 90 {
                    b.valid_from = old.valid_from;
                }
            }
            self.collection(CONTROL).replace_one(doc!{"_id":&key},
                doc!{"_id":key,"document_schema_version":1_i64,"record":encode(&b)?,"expires_at":expires(b.observed_at+86_400)})
                .upsert(true).await.map_err(unavailable)?;
            result.push(b);
        }
        Ok(result)
    }
    pub(crate) async fn save_infrastructure(
        &self,
        value: &Infrastructure,
        revision: u64,
    ) -> Result<()> {
        self.collection(CONTROL).replace_one(doc!{"_id":"infrastructure"},
            doc!{"_id":"infrastructure","document_schema_version":1_i64,"revision":revision as i64,"record":encode(value)?})
            .upsert(true).await.map_err(unavailable)?;
        Ok(())
    }
    pub(crate) async fn infrastructure_at(
        &self,
        session: &mut mongodb::ClientSession,
        revision: u64,
        at: u64,
    ) -> Result<Infrastructure> {
        let Some(d) = self
            .collection(CONTROL)
            .find_one(doc! {"_id":"infrastructure","revision":revision as i64})
            .max_time(DB_DEADLINE)
            .session(session)
            .await
            .map_err(unavailable)?
        else {
            return Ok(Infrastructure {
                state: "source_unavailable".into(),
                ..Default::default()
            });
        };
        let mut value: Infrastructure = decode(&d)?;
        if value.observed_at.is_none_or(|t| at.saturating_sub(t) > 90) {
            value.state = "stale".into();
        }
        Ok(value)
    }
}
