//! Isolated, bounded reporting pool and disposable immutable report snapshots.
mod admission;
mod allocation_deadline;
pub(super) use allocation_deadline::{allocation_operation, ALLOCATION_DB_DEADLINE};
mod budget;
mod control;
mod controlled_updates;
mod diagnostics;
mod initial_snapshot;
pub(crate) mod inventory;
mod on_demand_views;
mod options;
mod owner_allocation;
mod owner_names;
mod owners;
mod ownership;
mod progress;
pub(crate) use progress::Progress;
mod reads;
mod schema;
mod submission;
mod view_admission;
mod view_authority;
mod view_finish;
mod views;
mod work;
pub(crate) use cma_am_reporting::{chart_documents, unavailable};
use cma_am_reporting::{decode, encode, expires};
pub(crate) use views::ViewLease;
pub(crate) use work::ReportLease;

use crate::store::transaction_scope as tx;
use layer2_core::sandbox_metrics::{MetricsError, Result};
use mongodb::{
    bson::{doc, Document},
    options::ClientOptions,
    Client, Collection, Database,
};
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
    notifications: Arc<submission::ReportNotifications>,
    pub(crate) view_policy: Arc<crate::config::report_views::ViewPolicy>,
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

    #[cfg(test)]
    pub(crate) async fn bulk_updates(
        &self,
        collection: &str,
        updates: Vec<Document>,
    ) -> Result<()> {
        self.bulk_updates_in_session(collection, updates, None)
            .await
            .map_err(tx::into_domain)
    }

    pub(super) async fn bulk_updates_in_session(
        &self,
        collection: &str,
        updates: Vec<Document>,
        session: Option<&mut mongodb::ClientSession>,
    ) -> tx::Result<()> {
        self.bulk_updates_with_deadline(collection, updates, session, DB_DEADLINE)
            .await
    }

    pub(crate) async fn allocation_cache_updates(&self, updates: Vec<Document>) -> Result<()> {
        self.bulk_updates_with_deadline(
            super::sandbox_usage_rollups::COLLECTION,
            updates,
            None,
            ALLOCATION_DB_DEADLINE,
        )
        .await
        .map_err(tx::into_domain)
    }

    async fn bulk_updates_with_deadline(
        &self,
        collection: &str,
        updates: Vec<Document>,
        session: Option<&mut mongodb::ClientSession>,
        deadline: Duration,
    ) -> tx::Result<()> {
        if updates.len() > 200 {
            return Err(MetricsError::Busy.into());
        }
        let command = self.db.run_command(
            doc! {"update":collection,"updates":updates,"ordered":false,"maxTimeMS":deadline.as_millis() as i64},
        );
        let result = match session {
            Some(session) => command.session(session).await,
            None => command.await,
        }
        .map_err(tx::mongo)?;
        if result
            .get_array("writeErrors")
            .is_ok_and(|errors| !errors.is_empty())
            || result.contains_key("writeConcernError")
        {
            return Err(tx::ErrorKind::Command(result));
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) async fn connect(uri: &str, database: &str, key: &[u8]) -> Result<Arc<Self>> {
        Self::connect_with_budgets(
            uri,
            database,
            key,
            Default::default(),
            crate::config::sandbox_metrics::DEFAULT_REPORTING_POOL_SIZE,
        )
        .await
    }
    pub(crate) async fn connect_with_budgets(
        uri: &str,
        database: &str,
        key: &[u8],
        budgets: crate::config::sandbox_metrics::MetricsBudgets,
        pool_size: u32,
    ) -> Result<Arc<Self>> {
        let mut options = ClientOptions::parse(uri).await.map_err(unavailable)?;
        super::transaction_barrier::bind(&mut options, database);
        configure_pool(&mut options, pool_size);
        let circuit = Arc::default();
        budget::observe(&mut options, Arc::clone(&circuit));
        let client = Client::with_options(options).map_err(unavailable)?;
        tracing::info!(
            max_pool_size = pool_size,
            "reporting MongoDB pool configured"
        );
        let store = Arc::new(Self {
            budgets,
            db: client.database(database),
            client,
            signer: Arc::new(layer2_core::sandbox_metrics::cursor::CursorSigner::new(key)),
            circuit,
            notifications: Arc::default(),
            view_policy: Arc::default(),
        });
        store.initialize().await?;
        Ok(store)
    }
    pub(crate) fn collection(&self, name: &str) -> Collection<Document> {
        self.db.collection(name)
    }
    pub(crate) fn check_circuit(&self, now: u64) -> Result<()> {
        budget::check(&self.circuit, now)
    }
}

fn configure_pool(options: &mut ClientOptions, pool_size: u32) {
    options.max_pool_size = Some(pool_size);
    options.min_pool_size = Some(0);
    options.max_connecting = Some(1);
    options.connect_timeout = Some(DB_DEADLINE);
    options.server_selection_timeout = Some(DB_DEADLINE);
}
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod pool_tests;
