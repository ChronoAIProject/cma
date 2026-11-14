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
        let mut selection = input.validate(now)?;
        if !admin && selection.query.filters.owner.is_some() {
            return Err(MetricsError::Invalid(
                "Owner parameters are forbidden on owner routes.".into(),
            ));
        }
        if let Some(agent) = &selection.query.filters.agent_id {
            let sandbox = self.store.resolve_agent(&scope, agent).await?;
            if selection
                .query
                .filters
                .sandbox_id
                .as_ref()
                .is_some_and(|id| id != &sandbox)
            {
                return Err(MetricsError::Invalid(
                    "Agent and Sandbox selections disagree.".into(),
                ));
            }
            selection.query.filters.sandbox_id = Some(sandbox);
        }
        if let Some(id) = &selection.query.filters.sandbox_id {
            self.store.authorize_sandbox(&scope, id).await?;
        }
        self.store.check_read_budget()?;
        let view = self.store.latest(&scope, &selection, now).await?;
        if view.schema_version != 1
            || view.window != selection.window
            || view.requested_range != selection.query.range()
            || !selection.matches(&scope, &view.report)?
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
        self.store.reauthorize(&view.report).await?;
        self.store.verify_snapshot(&view.report).await?;
        Ok(view)
    }
}
