use super::*;
use async_trait::async_trait;
use layer2_core::sandbox_metrics::{
    MetricsStore, PageQuery, Report, ReportQuery, Row, Rows, Scope, Series, SeriesQuery,
};

#[async_trait]
impl MetricsStore for ReportingStore {
    async fn initial_snapshot_generation(&self, report: &Report) -> Result<u64> {
        self.verify_initial_snapshot(report).await
    }
    async fn latest(
        &self,
        scope: &Scope,
        selection: &layer2_core::sandbox_metrics::ViewSelection,
        at: u64,
    ) -> Result<layer2_core::sandbox_metrics::ReportView> {
        self.latest_view(scope, selection, at).await
    }
    async fn verify_snapshot(&self, report: &Report) -> Result<()> {
        self.verify_retained_snapshot(report).await
    }

    fn check_read_budget(&self) -> Result<()> {
        self.check_circuit(now())
    }
    async fn owner_allocations(
        &self,
        report: &Report,
        query: layer2_core::sandbox_metrics::OwnerAllocationQuery,
        owner_context: layer2_core::sandbox_metrics::OwnerContextAccess,
    ) -> Result<layer2_core::sandbox_metrics::OwnerAllocations> {
        self.read_owner_allocations(report, query, owner_context)
            .await
    }
    async fn options(
        &self,
        scope: &Scope,
        query: layer2_core::sandbox_metrics::OptionQuery,
        owner_context: layer2_core::sandbox_metrics::OwnerContextAccess,
    ) -> Result<layer2_core::sandbox_metrics::FilterOptions> {
        self.read_options(scope, query, owner_context).await
    }
    async fn resolve_agent(&self, scope: &Scope, id: &str) -> Result<String> {
        let mut filter = doc! {"_id":id};
        if !scope.admin {
            filter.insert("owner", &scope.principal);
        }
        let doc = self
            .collection("agents")
            .find_one(filter)
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::NotFound)?;
        let agent = super::super::agents::read_agent(&doc).map_err(unavailable)?;
        self.authorize_sandbox(scope, agent.sandbox_id.as_str())
            .await?;
        Ok(agent.sandbox_id.to_string())
    }
    async fn authorize_sandbox(&self, scope: &Scope, id: &str) -> Result<()> {
        let mut filter = doc! {"_id":id};
        if !scope.admin {
            filter.insert("owner", &scope.principal);
        }
        self.collection(super::super::sandbox_usage::STATES)
            .find_one(filter)
            .projection(doc! {"_id":1})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::NotFound)?;
        let mut current = self
            .collection("sandboxes")
            .find_one(doc! {"_id":id})
            .projection(doc! {"owner":1})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        if !scope.admin
            && current
                .as_mut()
                .is_some_and(|d| d.get_str("owner").ok() != Some(scope.principal.as_str()))
        {
            return Err(MetricsError::NotFound);
        }
        Ok(())
    }
    async fn submit(&self, scope: Scope, query: ReportQuery, at: u64) -> Result<Report> {
        self.submit_with_completion(scope, query, at).await
    }
    async fn report(&self, scope: &Scope, id: &str, at: u64) -> Result<Report> {
        self.persistence.read_report(scope, id, at).await
    }
    async fn reauthorize(&self, report: &Report) -> Result<()> {
        self.verify_retained_snapshot(report).await?;
        self.persistence
            .check_report_membership(
                self.collection(ROWS),
                super::super::sandbox_usage::STATES,
                report,
            )
            .await?;
        if let Some(id) = &report.query.filters.sandbox_id {
            self.authorize_sandbox(&report.scope, id).await?;
        }
        if let Some(id) = &report.query.filters.agent_id {
            let current = self.resolve_agent(&report.scope, id).await?;
            if report.query.filters.sandbox_id.as_deref() != Some(current.as_str()) {
                return Err(MetricsError::NotFound);
            }
        }
        self.verify_retained_snapshot(report).await?;
        Ok(())
    }
    async fn page(&self, report: &Report, query: PageQuery) -> Result<Rows> {
        let _timer = self.time_read("report_page")?;
        let mut page = cma_am_reporting::read_row_page(
            self.collection(ROWS),
            &self.signer,
            report,
            query,
            DB_DEADLINE,
        )
        .await?;
        self.hydrate_owner_names(page.items.iter_mut().map(|row| &mut row.owner).collect())
            .await?;
        Ok(page)
    }
    async fn detail(&self, report: &Report, id: &str) -> Result<Row> {
        let mut row =
            cma_am_reporting::read_row(self.collection(ROWS), report, id, DB_DEADLINE).await?;
        self.hydrate_owner_names(vec![&mut row.owner]).await?;
        Ok(row)
    }
    async fn owners(
        &self,
        report: &Report,
        query: layer2_core::sandbox_metrics::OwnerQuery,
    ) -> Result<layer2_core::sandbox_metrics::Owners> {
        self.read_owners(report, query).await
    }
    async fn series(&self, report: &Report, query: SeriesQuery) -> Result<Series> {
        cma_am_reporting::read_series(self.collection(CHARTS), report, query, DB_DEADLINE).await
    }
}
