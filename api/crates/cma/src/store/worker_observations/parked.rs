//! Bounded indexed projections; no log, message, identity or credential reads.
use crate::sandbox::work_retry::{agent_class, has_probe_delay, sandbox_step, sandbox_step_class};
use futures_util::TryStreamExt;
use layer2_core::sandbox::work_retry::{WorkRetryClass, WORK_RETRY_EXTERNAL_BUDGET};
use layer2_core::sandbox::{AgentWorkStep, SandboxWorkStep};
use mongodb::{
    bson::{self, doc, Document},
    Database,
};
use std::{collections::BTreeMap, time::Duration};

const ROW_LIMIT: i64 = 1000;
type Counts = BTreeMap<&'static str, u64>;

pub(super) struct ParkedObserver {
    database: Database,
    previous: BTreeMap<&'static str, Counts>,
}

impl ParkedObserver {
    pub(super) fn new(database: Database) -> Self {
        for worker in ["agents", "sandboxes"] {
            super::initialize(worker, "parked");
        }
        Self {
            database,
            previous: BTreeMap::new(),
        }
    }

    pub(super) async fn sample(&mut self) {
        for worker in ["agents", "sandboxes"] {
            let result = tokio::time::timeout(Duration::from_secs(2), self.read(worker)).await;
            let Ok(Ok(counts)) = result else {
                super::sampled(worker, "parked", false);
                continue;
            };
            let previous = self.previous.entry(worker).or_default();
            for step in previous.keys().chain(counts.keys()) {
                metrics::gauge!("cma_work_parked_items", "worker" => worker, "step" => *step)
                    .set(*counts.get(step).unwrap_or(&0) as f64);
            }
            metrics::gauge!("cma_worker_durable_items", "worker" => worker, "state" => "parked")
                .set(counts.values().sum::<u64>() as f64);
            *previous = counts;
            super::sampled(worker, "parked", true);
        }
    }

    async fn read(&self, worker: &str) -> Result<Counts, ()> {
        let hint = if worker == "agents" {
            mongodb::options::Hint::Name("agents_due_work".into())
        } else {
            mongodb::options::Hint::Keys(doc! {
                "work_pending": 1, "work_ready_at": 1, "work_lease_expires_at": 1
            })
        };
        let mut rows = crate::store::durability::majority_collection::<Document>(&self.database, worker)
            .find(doc! { "work_pending": true })
            .hint(hint)
            .projection(doc! { "_id": 0, "record.work.step": 1, "record.work.attempts": 1,
                "record.work.ready_at": 1, "record.updated_at": 1, "record.lifecycle.operation.purpose": 1 })
            .limit(ROW_LIMIT + 1)
            .batch_size(100)
            .max_time(Duration::from_secs(1))
            .await.map_err(|_| ())?;
        let mut counts = Counts::new();
        let mut count = 0;
        while let Some(row) = rows.try_next().await.map_err(|_| ())? {
            count += 1;
            // Do not publish a partial population as an exact parked count.
            if count > ROW_LIMIT {
                return Err(());
            }
            if let Some(step) = parked_step(worker, &row)? {
                *counts.entry(step).or_default() += 1;
            }
        }
        Ok(counts)
    }
}

fn parked_step(worker: &str, row: &Document) -> Result<Option<&'static str>, ()> {
    let record = row.get_document("record").map_err(|_| ())?;
    let work = record.get_document("work").map_err(|_| ())?;
    let attempts: u32 =
        bson::deserialize_from_bson(work.get("attempts").ok_or(())?.clone()).map_err(|_| ())?;
    let updated = record.get_str("updated_at").map_err(|_| ())?;
    let ready = work.get_str("ready_at").map_err(|_| ())?;
    layer2_core::seconds_from_rfc3339(updated).ok_or(())?;
    layer2_core::seconds_from_rfc3339(ready).ok_or(())?;
    if attempts <= WORK_RETRY_EXTERNAL_BUDGET || !has_probe_delay(updated, ready) {
        return Ok(None);
    }
    let value = work.get("step").ok_or(())?.clone();
    let (label, class) = if worker == "agents" {
        let step: AgentWorkStep = bson::deserialize_from_bson(value).map_err(|_| ())?;
        (step.operation_name(), agent_class(step))
    } else {
        let step: SandboxWorkStep = bson::deserialize_from_bson(value).map_err(|_| ())?;
        let reviving = record
            .get_document("lifecycle")
            .ok()
            .and_then(|lifecycle| lifecycle.get_document("operation").ok())
            .and_then(|operation| operation.get_str("purpose").ok())
            == Some("revive");
        (sandbox_step(step), sandbox_step_class(step, reviving))
    };
    Ok((class == WorkRetryClass::Maintenance).then_some(label))
}

#[cfg(test)]
#[path = "parked_tests.rs"]
mod tests;
