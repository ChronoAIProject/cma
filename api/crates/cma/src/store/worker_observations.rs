//! Read-only durable summaries, cached outside the scrape path. Never inspect inputs.
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Bson, DateTime, Document},
    Database,
};
use std::time::Duration;

mod parked;

pub(crate) struct Observations(
    Database,
    bool,
    Option<(
        crate::telemetry::task_cohort::Cache,
        crate::config::task_audit::Selection,
    )>,
);
impl Observations {
    pub(super) fn new(database: Database) -> Self {
        Self(database, false, None)
    }

    pub(crate) fn with_work_retry_policy(mut self, enabled: bool) -> Self {
        self.1 = enabled;
        self
    }

    pub(crate) fn with_task_audit(
        mut self,
        cache: crate::telemetry::task_cohort::Cache,
        selection: Option<crate::config::task_audit::Selection>,
    ) -> Self {
        self.2 = selection.map(|selection| {
            cache.initialize();
            (cache, selection)
        });
        self
    }

    pub(crate) fn spawn(
        self,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        for spec in specifications(DateTime::now()) {
            initialize(spec.worker, spec.state);
        }
        initialize("native_warm_pools", "due");
        tokio::spawn(async move {
            let mut parked = self.1.then(|| parked::ParkedObserver::new(self.0.clone()));
            let audit = self.2.as_ref().map(|(cache, selection)| {
                let store = super::sandbox_mongo::MongoSandboxStore::audit_handles(
                    self.0.client(),
                    self.0.name(),
                );
                (cache, selection, store)
            });
            let mut next_audit = tokio::time::Instant::now();
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => return,
                    _ = tick.tick() => {}
                }
                for spec in specifications(DateTime::now()) {
                    let result = tokio::select! {
                        _ = crate::shutdown::signalled(shutdown.clone()) => return,
                        result = tokio::time::timeout(Duration::from_secs(2), self.read(&spec)) => result
                    };
                    match result {
                        Ok(Ok(summary)) => {
                            if spec.worker == "agent_commands" && spec.state != "expired_queue" {
                                metrics::gauge!("cma_admission_durable_commands", "status" => spec.state).set(summary.count as f64);
                            }
                            metrics::gauge!("cma_worker_durable_items", "worker" => spec.worker, "state" => spec.state).set(summary.count as f64);
                            metrics::gauge!("cma_worker_durable_oldest_timestamp_seconds", "worker" => spec.worker, "state" => spec.state).set(summary.oldest);
                            sampled(spec.worker, spec.state, true);
                        }
                        _ => sampled(spec.worker, spec.state, false),
                    }
                }
                if let Some((cache, selection, store)) = &audit {
                    if tokio::time::Instant::now() >= next_audit {
                        let result = tokio::select! {
                            _ = crate::shutdown::signalled(shutdown.clone()) => return,
                            result = super::task_audit::observe(store, selection) => result
                        };
                        cache.update(result.map(|report| report.metrics()));
                        next_audit = tokio::time::Instant::now() + Duration::from_secs(60);
                    }
                }
                if let Some(parked) = parked.as_mut() {
                    tokio::select! {
                        _ = crate::shutdown::signalled(shutdown.clone()) => return,
                        _ = parked.sample() => {}
                    }
                }
            }
        })
    }

    async fn read(&self, spec: &Spec) -> Result<Summary, ()> {
        let mut rows = super::durability::majority_collection::<Document>(&self.0, spec.collection)
            .aggregate([
                doc! { "$match": spec.filter.clone() },
                doc! { "$group": { "_id": Bson::Null, "count": { "$sum": 1 }, "oldest": { "$min": spec.at.clone() } } },
            ])
            .hint(mongodb::options::Hint::Name(match spec.worker {
                "agent_commands" => "agent_command_observations",
                "sandbox_commands" => "sandbox_control_observations",
                "launches" => "trigger_launches_observations",
                _ => "status_1_retry_at_1_created_at_1",
            }.into()))
            .max_time(Duration::from_secs(1))
            .allow_disk_use(false)
            .await.map_err(|_| ())?;
        let Some(row) = rows.try_next().await.map_err(|_| ())? else {
            return Ok(Summary {
                count: 0,
                oldest: 0.,
            });
        };
        decode(&row)
    }
}

struct Summary {
    count: u64,
    oldest: f64,
}
fn decode(row: &Document) -> Result<Summary, ()> {
    let count = row
        .get("count")
        .and_then(Bson::as_i64)
        .or_else(|| row.get("count").and_then(Bson::as_i32).map(i64::from))
        .ok_or(())?;
    let count = u64::try_from(count).map_err(|_| ())?;
    let oldest = row
        .get_datetime("oldest")
        .map_err(|_| ())?
        .timestamp_millis() as f64
        / 1000.;
    Ok(Summary { count, oldest })
}

fn initialize(worker: &'static str, state: &'static str) {
    metrics::gauge!("cma_worker_durable_sample_success", "worker" => worker, "state" => state)
        .set(0.);
    metrics::gauge!("cma_worker_durable_sample_timestamp_seconds", "worker" => worker, "state" => state).set(0.);
    metrics::gauge!("cma_worker_durable_sample_success_timestamp_seconds", "worker" => worker, "state" => state).set(0.);
}
pub(crate) fn sampled(worker: &'static str, state: &'static str, success: bool) {
    let at = crate::telemetry::timestamp();
    metrics::gauge!("cma_worker_durable_sample_success", "worker" => worker, "state" => state)
        .set(f64::from(success));
    metrics::gauge!("cma_worker_durable_sample_timestamp_seconds", "worker" => worker, "state" => state).set(at);
    if success {
        metrics::gauge!("cma_worker_durable_sample_success_timestamp_seconds", "worker" => worker, "state" => state).set(at);
    }
}

struct Spec {
    worker: &'static str,
    state: &'static str,
    collection: &'static str,
    filter: Document,
    at: Bson,
}
fn date(field: &str) -> Bson {
    doc! { "$toDate": field }.into()
}
fn specifications(now: DateTime) -> Vec<Spec> {
    let timestamp = layer2_core::rfc3339_at(now.to_system_time());
    let mut specs = Vec::new();
    for state in [
        "queued",
        "in_progress",
        "waiting_action",
        "completed",
        "failed",
        "cancelled",
    ] {
        specs.push(Spec {
            worker: "agent_commands",
            state,
            collection: "agent_commands",
            filter: doc! { "status": state },
            at: date("$record.receipt.admitted_at"),
        });
    }
    specs.push(Spec {
        worker: "agent_commands",
        state: "expired_queue",
        collection: "agent_commands",
        filter: doc! { "status": "queued", "queue_deadline": { "$lte": &timestamp } },
        at: date("$queue_deadline"),
    });
    specs.push(Spec {
        worker: "sandbox_commands",
        state: "queued",
        collection: "sandbox_controls",
        filter: doc! { "queued": true },
        at: date("$record.admitted_at"),
    });
    specs.push(Spec {
        worker: "sandbox_commands",
        state: "expired_queue",
        collection: "sandbox_controls",
        filter: doc! { "queued": true, "queue_deadline": { "$lte": &timestamp } },
        at: date("$queue_deadline"),
    });
    specs.push(Spec { worker: "agents", state: "lifecycle_due", collection: super::agent_lifecycle_operations::COLLECTION, filter: super::agent_lifecycle_operations::pending_filter(now), at: doc! { "$max": ["$created_at", { "$cond": [{ "$eq": ["$status", "queued"] }, "$retry_at", "$lease_expires_at"] }] }.into() });
    specs.push(Spec {
        worker: "launches",
        state: "due",
        collection: "trigger_launches",
        filter: super::trigger_definitions::due_filter(&timestamp),
        at: doc! { "$max": [date("$work_ready_at"), date("$work_lease_expires_at")] }.into(),
    });
    specs
}

#[cfg(test)]
mod tests;
