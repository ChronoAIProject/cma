use super::*;
use async_trait::async_trait;
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{
    MetricsStore, PageQuery, Report, ReportQuery, Row, Rows, Scope, Series, SeriesQuery,
};

#[async_trait]
impl MetricsStore for ReportingStore {
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
        let d = self
            .collection(JOBS)
            .find_one(doc! {"_id":id,"principal":&scope.principal,"admin":scope.admin})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::NotFound)?;
        let report: Report = decode(&d)?;
        if report.expires_at <= at
            || d.get_i64("retire_after")
                .is_ok_and(|until| until <= at as i64)
        {
            return Err(MetricsError::Expired);
        }
        if report.status == layer2_core::sandbox_metrics::ReportStatus::Failed {
            return Err(MetricsError::Unavailable);
        }
        Ok(report)
    }
    async fn reauthorize(&self, report: &Report) -> Result<()> {
        self.verify_retained_snapshot(report).await?;
        // Even admin cached membership must not retain another owner's old
        // attribution after a transfer. Owner changes invalidate the snapshot.
        let pipeline = [
            doc! {"$match":{"report_id":&report.report_id}},
            doc! {"$lookup":{"from":super::super::sandbox_usage::STATES,"localField":"sandbox_id","foreignField":"_id","as":"current"}},
            doc! {"$lookup":{"from":"sandboxes","localField":"sandbox_id","foreignField":"_id","as":"sandbox"}},
            doc! {"$match":{"$expr":{"$or":[
                {"$ne":["$owner",{"$arrayElemAt":["$current.owner",0]}]},
                {"$and":[{"$gt":[{"$size":"$sandbox"},0]},
                    {"$ne":["$owner",{"$arrayElemAt":["$sandbox.owner",0]}]}]}
            ]}}},
            doc! {"$limit":1},
            doc! {"$project":{"_id":1}},
        ];
        let mut cursor = self
            .collection(ROWS)
            .aggregate(pipeline)
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        if cursor.try_next().await.map_err(unavailable)?.is_some() {
            return Err(MetricsError::NotFound);
        }
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
