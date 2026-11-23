use super::*;

impl MetricsService {
    pub async fn latest(
        &self,
        caller: &Caller,
        admin: bool,
        input: ViewInput,
        now: u64,
    ) -> Result<ReportView> {
        let scope = authorize(caller, admin)?;
        let view = self.select_latest(&scope, input, now).await?;
        self.store.reauthorize(&view.report).await?;
        self.store.verify_snapshot(&view.report).await?;
        Ok(view)
    }

    pub(super) async fn select_latest(
        &self,
        scope: &Scope,
        input: ViewInput,
        now: u64,
    ) -> Result<ReportView> {
        let mut selection = input.validate(now)?;
        self.select_resources(scope, &mut selection.query).await?;
        self.store.check_read_budget()?;
        let view = self.store.latest(scope, &selection, now).await?;
        if view.schema_version != 1
            || view.window != selection.window
            || view.requested_range != selection.query.range()
            || !selection.matches(scope, &view.report)?
        {
            return Err(MetricsError::Unavailable);
        }
        match view.report.status {
            ReportStatus::Ready
                if view.report.report_complete
                    && view.report.summary.is_some()
                    && view.generated_at.is_some_and(|at| {
                        at >= view.report.as_of && at < view.report.expires_at
                    }) => {}
            ReportStatus::Pending
                if view.generated_at.is_none()
                    && !view.stale
                    && view.refresh.state == RefreshState::Pending
                    && view.refresh.report_id.as_deref() == Some(&view.report.report_id) => {}
            _ => return Err(MetricsError::Unavailable),
        }
        Ok(view)
    }
}
