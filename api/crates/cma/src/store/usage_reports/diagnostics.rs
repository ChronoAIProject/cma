//! Short-lived physical proof continuity, separate from customer assignments.
use super::*;
use crate::store::transaction_scope as tx;
use layer2_core::sandbox_metrics::{Binding, Infrastructure};
use std::collections::BTreeSet;
impl ReportingStore {
    pub(crate) async fn diagnostic_bindings(
        &self,
        holder: &str,
        candidates: Vec<Binding>,
    ) -> Result<Vec<Binding>> {
        if candidates.len() > 200 {
            return Err(MetricsError::Busy);
        }
        self.check_circuit(now())?;
        tx::run(
            &self.client,
            (self, holder, &candidates),
            |&(store, holder, candidates), session| {
                Box::pin(async move {
                    let bindings = store
                        .diagnostic_bindings_in_session(candidates, session)
                        .await?;
                    store
                        .fence_control_transaction("diagnostics", holder, session)
                        .await?;
                    Ok(bindings)
                })
            },
        )
        .await
    }

    async fn diagnostic_bindings_in_session(
        &self,
        candidates: &[Binding],
        session: &mut mongodb::ClientSession,
    ) -> tx::Result<Vec<Binding>> {
        let keys: Vec<_> = candidates.iter().map(Binding::physical_key).collect();
        let mut cursor = self
            .collection(super::super::sandbox_metric_bindings::COLLECTION)
            .find(doc! {"active_physical":{"$in":keys}})
            .projection(doc! {"active_physical":1})
            .session(&mut *session)
            .max_time(DB_DEADLINE)
            .await
            .map_err(tx::mongo)?;
        let mut assigned = BTreeSet::new();
        while let Some(d) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(tx::mongo)?
        {
            assigned.insert(
                d.get_str("active_physical")
                    .map_err(unavailable)?
                    .to_owned(),
            );
        }
        let mut result = Vec::new();
        for mut b in candidates.iter().cloned() {
            if assigned.contains(&b.physical_key()) {
                continue;
            }
            let key = format!("diagnostic:{}", b.physical_key());
            if let Some(d) = self
                .collection(CONTROL)
                .find_one(doc! {"_id":&key})
                .session(&mut *session)
                .max_time(DB_DEADLINE)
                .await
                .map_err(tx::mongo)?
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
                .upsert(true).session(&mut *session).await.map_err(tx::mongo)?;
            result.push(b);
        }
        Ok(result)
    }
    pub(crate) async fn save_infrastructure(
        &self,
        holder: &str,
        value: &Infrastructure,
        revision: u64,
    ) -> Result<()> {
        self.check_circuit(now())?;
        tx::run(&self.client, (self, holder, value, revision), |&(store, holder, value, revision), session| {
            Box::pin(async move {
                store.collection(CONTROL).replace_one(doc!{"_id":"infrastructure"},
                    doc!{"_id":"infrastructure","document_schema_version":1_i64,"revision":revision as i64,"record":encode(value)?})
                    .upsert(true).session(&mut *session).await.map_err(tx::mongo)?;
                store.fence_control_transaction("diagnostics", holder, session).await
            })
        }).await
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
