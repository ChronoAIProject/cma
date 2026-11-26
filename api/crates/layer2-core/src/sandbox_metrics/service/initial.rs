use super::*;
use crate::sandbox_metrics::initial::{
    validate_initial_report, validate_initial_rows, validate_initial_series,
};

// This grant never leaves the invocation and cannot be cloned or serialized.
struct InitialGrant<'a> {
    store: &'a dyn MetricsStore,
    caller: &'a Caller,
    report: &'a Report,
    projection: &'a InitialProjection,
    generation: u64,
    started: tokio::time::Instant,
    at: u64,
}

impl MetricsService {
    pub async fn initial_report(
        &self,
        caller: &Caller,
        admin: bool,
        id: &str,
        projection: InitialProjection,
        at: u64,
    ) -> Result<InitialReport> {
        let started = tokio::time::Instant::now();
        let scope = authorize(caller, admin)?;
        projection.validate(admin)?;
        if !query::opaque_id(id, "umr_") {
            return Err(MetricsError::NotFound);
        }
        self.store.check_read_budget()?;
        let report = self.store.report(&scope, id, at).await?;
        if report.scope != scope || report.report_id != id {
            return Err(MetricsError::NotFound);
        }
        let initial = self
            .initial_data(caller, &report, &projection, started, at)
            .await?;
        Ok(InitialReport { report, initial })
    }

    pub async fn submit_initial(
        &self,
        caller: &Caller,
        admin: bool,
        input: InitialReportInput,
        at: u64,
    ) -> Result<InitialReport> {
        let started = tokio::time::Instant::now();
        let scope = authorize(caller, admin)?;
        input.projection.validate(admin)?;
        let mut query = input.query.validate(at)?;
        self.select_resources(&scope, &mut query).await?;
        self.store.check_read_budget()?;
        let report = self.store.submit(scope.clone(), query.clone(), at).await?;
        if report.scope != scope {
            return Err(MetricsError::NotFound);
        }
        if report.query != query {
            return Err(MetricsError::Unavailable);
        }
        let initial = self
            .initial_data(caller, &report, &input.projection, started, at)
            .await?;
        Ok(InitialReport { report, initial })
    }

    pub async fn latest_initial(
        &self,
        caller: &Caller,
        admin: bool,
        input: InitialViewInput,
        at: u64,
    ) -> Result<InitialView> {
        let started = tokio::time::Instant::now();
        let scope = authorize(caller, admin)?;
        input.projection.validate(admin)?;
        let view = self.select_latest(&scope, input.selection, at).await?;
        let initial = self
            .initial_data(caller, &view.report, &input.projection, started, at)
            .await?;
        Ok(InitialView { view, initial })
    }

    async fn initial_data(
        &self,
        caller: &Caller,
        report: &Report,
        projection: &InitialProjection,
        started: tokio::time::Instant,
        at: u64,
    ) -> Result<Option<InitialData>> {
        if !query::opaque_id(&report.report_id, "umr_") {
            return Err(MetricsError::Unavailable);
        }
        if report.expires_at <= at {
            return Err(MetricsError::Expired);
        }
        if report.status == ReportStatus::Pending {
            if report.schema_version != 1 || report.report_complete || report.summary.is_some() {
                return Err(MetricsError::Unavailable);
            }
            self.store.reauthorize(report).await?;
            if at.saturating_add(started.elapsed().as_secs()) >= report.expires_at {
                return Err(MetricsError::Expired);
            }
            return Ok(None);
        }
        if report.status != ReportStatus::Ready {
            return Err(MetricsError::Unavailable);
        }
        validate_initial_report(report)?;
        let generation = self.store.initial_snapshot_generation(report).await?;
        if generation == 0 {
            return Err(MetricsError::Unavailable);
        }
        self.store.reauthorize(report).await?;
        InitialGrant {
            store: self.store.as_ref(),
            caller,
            report,
            projection,
            generation,
            started,
            at,
        }
        .read()
        .await
        .map(Some)
    }
}

impl InitialGrant<'_> {
    async fn read(self) -> Result<InitialData> {
        let rows = self
            .store
            .page(self.report, self.projection.page_query())
            .await?;
        validate_initial_rows(self.report, &self.projection.first_page, &rows)?;
        let mut charts = Vec::with_capacity(self.projection.total_metrics.len());
        for &metric in &self.projection.total_metrics {
            let chart = self
                .store
                .series(
                    self.report,
                    SeriesQuery {
                        metric,
                        group_by: GroupBy::Total,
                        series_id: None,
                        top: None,
                    },
                )
                .await?;
            validate_initial_series(self.report, metric, &chart)?;
            charts.push(chart);
        }
        if self.store.initial_snapshot_generation(self.report).await? != self.generation
            || authorize(self.caller, self.report.scope.admin)? != self.report.scope
        {
            return Err(MetricsError::Unavailable);
        }
        if self.at.saturating_add(self.started.elapsed().as_secs()) >= self.report.expires_at {
            return Err(MetricsError::Expired);
        }
        Ok(InitialData {
            projection: self.projection.clone(),
            rows,
            charts,
        })
    }
}
