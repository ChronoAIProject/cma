//! Isolated, bounded reporting pool and disposable immutable report snapshots.
mod admission;
mod budget;
mod charts;
mod diagnostics;
pub(crate) mod inventory;
mod options;
mod owner_allocation;
mod owners;
mod progress;
pub(crate) use progress::Progress;
mod reads;
mod schema;
mod work;
pub(crate) use charts::chart_documents;
pub(crate) use work::ReportLease;

use layer2_core::sandbox_metrics::{MetricsError, Result};
use mongodb::{
    bson::{doc, Document},
    options::ClientOptions,
    Client, Collection, Database,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{sync::Arc, time::Duration};

pub(crate) const JOBS: &str = "sandbox_usage_reports";
pub(crate) const ROWS: &str = "sandbox_usage_report_rows";
pub(crate) const CHARTS: &str = "sandbox_usage_report_charts";
pub(crate) const CONTROL: &str = "sandbox_usage_report_control";
pub(super) const DB_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub(crate) struct ReportingStore {
    pub(crate) budgets: crate::config::sandbox_metrics::MetricsBudgets,
    pub(super) client: Client,
    pub(super) db: Database,
    pub(super) signer: Arc<layer2_core::sandbox_metrics::cursor::CursorSigner>,
    pub(super) circuit: Arc<std::sync::atomic::AtomicU64>,
}
impl ReportingStore {
    pub(crate) async fn snapshot(&self) -> Result<mongodb::ClientSession> {
        self.check_circuit(now())?;
        self.client
            .start_session()
            .snapshot(true)
            .await
            .map_err(unavailable)
    }

    pub(crate) async fn bulk_updates(
        &self,
        collection: &str,
        updates: Vec<Document>,
    ) -> Result<()> {
        if updates.len() > 200 {
            return Err(MetricsError::Busy);
        }
        let result = self
            .db
            .run_command(
                doc! {"update":collection,"updates":updates,"ordered":false,"maxTimeMS":2000_i64},
            )
            .await
            .map_err(unavailable)?;
        if result
            .get_array("writeErrors")
            .is_ok_and(|errors| !errors.is_empty())
            || result.contains_key("writeConcernError")
        {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) async fn connect(uri: &str, database: &str, key: &[u8]) -> Result<Arc<Self>> {
        Self::connect_with_budgets(uri, database, key, Default::default()).await
    }
    pub(crate) async fn connect_with_budgets(
        uri: &str,
        database: &str,
        key: &[u8],
        budgets: crate::config::sandbox_metrics::MetricsBudgets,
    ) -> Result<Arc<Self>> {
        let mut options = ClientOptions::parse(uri).await.map_err(unavailable)?;
        options.max_pool_size = Some(4);
        options.min_pool_size = Some(0);
        options.max_connecting = Some(1);
        options.connect_timeout = Some(DB_DEADLINE);
        options.server_selection_timeout = Some(DB_DEADLINE);
        let circuit = Arc::default();
        budget::observe(&mut options, Arc::clone(&circuit));
        let client = Client::with_options(options).map_err(unavailable)?;
        let store = Arc::new(Self {
            budgets,
            db: client.database(database),
            client,
            signer: Arc::new(layer2_core::sandbox_metrics::cursor::CursorSigner::new(key)),
            circuit,
        });
        store.initialize().await?;
        Ok(store)
    }
    pub(super) fn collection(&self, name: &str) -> Collection<Document> {
        self.db.collection(name)
    }
    pub(crate) fn check_circuit(&self, now: u64) -> Result<()> {
        budget::check(&self.circuit, now)
    }
    pub(crate) async fn lease(&self, key: &str, holder: &str, seconds: u64) -> Result<bool> {
        let now = now();
        let result = self
            .collection(CONTROL)
            .find_one_and_update(
                doc! {"_id":key,"$or":[{"lease_until":{"$lte":now as i64}},{"holder":holder}]},
                doc! {"$set":{"holder":holder,"lease_until":(now+seconds) as i64}},
            )
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        Ok(result.is_some())
    }
    pub(crate) async fn release(&self, key: &str, holder: &str) -> Result<()> {
        self.collection(CONTROL)
            .update_one(
                doc! {"_id":key,"holder":holder},
                doc! {"$set":{"lease_until":0_i64}},
            )
            .await
            .map_err(unavailable)?;
        Ok(())
    }
}
pub(super) fn unavailable(error: impl std::fmt::Display) -> MetricsError {
    tracing::warn!(error=%error,"reporting store unavailable");
    MetricsError::Unavailable
}
pub(super) fn encode<T: Serialize>(record: &T) -> Result<mongodb::bson::Bson> {
    mongodb::bson::serialize_to_bson(record).map_err(unavailable)
}
pub(super) fn decode<T: DeserializeOwned>(document: &Document) -> Result<T> {
    if document.get_i64("document_schema_version").ok() != Some(1) {
        return Err(MetricsError::Unavailable);
    }
    mongodb::bson::deserialize_from_bson(
        document
            .get("record")
            .cloned()
            .ok_or(MetricsError::Unavailable)?,
    )
    .map_err(unavailable)
}
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub(super) fn expires(at: u64) -> mongodb::bson::DateTime {
    mongodb::bson::DateTime::from_millis((at * 1000) as i64)
}

#[cfg(test)]
mod tests;
