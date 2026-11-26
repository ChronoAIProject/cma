use super::*;
use async_trait::async_trait;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize},
    Mutex,
};
pub(super) const AT: u64 = 1_800_000_000;
pub(super) const REPORT: &str = "umr_00000000000000000000000000000001";
pub(super) const SANDBOX: &str = "sbx_00000000000000000000000000000001";

pub(super) struct Store {
    pub calls: Mutex<Vec<&'static str>>,
    pub revoked: AtomicBool,
    snapshots: AtomicUsize,
    fault: &'static str,
    pub report: Report,
}
impl Store {
    pub fn new(fault: &'static str, admin: bool) -> Self {
        let query = ReportInput {
            from: Some(AT - 3600),
            to: Some(AT - 120),
            ..Default::default()
        }
        .validate(AT)
        .unwrap();
        let mut report = Report {
            schema_version: 1,
            report_id: REPORT.into(),
            status: ReportStatus::Ready,
            report_complete: true,
            scope: Scope {
                principal: "owner".into(),
                admin,
            },
            effective_range: query.effective_range(AT),
            query,
            as_of: AT,
            measurement_cutoff: AT - 120,
            expires_at: AT + 600,
            allocation_watermark: "fixture".into(),
            mapping_revision: 1,
            available_from: None,
            tracking_started_at: None,
            warnings: vec![],
            source_health: "source_unavailable".into(),
            summary: Some(Summary {
                sandbox_count: 1,
                ..Default::default()
            }),
            poll_url: String::new(),
            retry_after_seconds: 2,
        };
        match fault {
            "scope" => report.scope.principal = "foreign".into(),
            "report-id" => report.report_id = "umr_00000000000000000000000000000002".into(),
            "descriptor-range" => report.effective_range.from += 1,
            "descriptor-step" => report.query.step_seconds = 0,
            "pending" => {
                report.status = ReportStatus::Pending;
                report.report_complete = false;
                report.summary = None;
            }
            "malformed-pending" => report.status = ReportStatus::Pending,
            "failed" => report.status = ReportStatus::Failed,
            "expires" => report.expires_at = AT + 2,
            _ => {}
        }
        Self {
            calls: Mutex::new(vec![]),
            revoked: AtomicBool::new(false),
            snapshots: AtomicUsize::new(0),
            fault,
            report,
        }
    }
    fn call(&self, name: &'static str) {
        self.calls.lock().unwrap().push(name);
    }
}
#[async_trait]
impl MetricsStore for Store {
    async fn initial_snapshot_generation(&self, _: &Report) -> Result<u64> {
        self.call("snapshot");
        let second = self.snapshots.fetch_add(1, Ordering::SeqCst) > 0;
        match self.fault {
            "snapshot-lost" if second => Err(MetricsError::Unavailable),
            "unsupported" => Err(MetricsError::Unavailable),
            "zero-generation" => Ok(0),
            "generation" if second => Ok(2),
            _ => Ok(1),
        }
    }
    async fn resolve_agent(&self, _: &Scope, _: &str) -> Result<String> {
        self.call("resolve");
        if self.revoked.load(Ordering::SeqCst) {
            return Err(MetricsError::NotFound);
        }
        Ok(SANDBOX.into())
    }
    async fn authorize_sandbox(&self, _: &Scope, _: &str) -> Result<()> {
        self.call("selected");
        if self.revoked.load(Ordering::SeqCst) {
            return Err(MetricsError::NotFound);
        }
        Ok(())
    }
    async fn submit(&self, _: Scope, query: ReportQuery, _: u64) -> Result<Report> {
        self.call("submit");
        let mut report = self.report.clone();
        if self.fault != "submit-query" {
            report.query = query;
            report.effective_range = report.query.effective_range(AT);
        }
        if self.fault == "post-submit-revocation" {
            self.revoked.store(true, Ordering::SeqCst);
        }
        Ok(report)
    }
    async fn latest(
        &self,
        scope: &Scope,
        selection: &ViewSelection,
        at: u64,
    ) -> Result<ReportView> {
        self.call("latest");
        if self.fault == "view-disabled" {
            return Err(MetricsError::ViewDisabled);
        }
        let report = self
            .submit(scope.clone(), selection.query.clone(), at)
            .await?;
        let pending = report.status == ReportStatus::Pending;
        let mut view = ReportView {
            schema_version: 1,
            window: selection.window,
            requested_range: selection.query.range(),
            generated_at: (!pending).then_some(AT),
            stale: false,
            refresh: ViewRefresh {
                state: if pending {
                    RefreshState::Pending
                } else {
                    RefreshState::Idle
                },
                report_id: pending.then(|| report.report_id.clone()),
                retry_after_seconds: 2,
            },
            report,
        };
        if self.fault == "view-range" {
            view.requested_range.from += 1;
        }
        Ok(view)
    }
    async fn report(&self, _: &Scope, _: &str, _: u64) -> Result<Report> {
        self.call("report");
        Ok(self.report.clone())
    }
    async fn reauthorize(&self, _: &Report) -> Result<()> {
        self.call("authorize");
        if self.revoked.load(Ordering::SeqCst) {
            Err(MetricsError::NotFound)
        } else {
            Ok(())
        }
    }
    async fn page(&self, report: &Report, page: PageQuery) -> Result<Rows> {
        self.call("page");
        assert_eq!(report.report_id, self.report.report_id);
        assert!(page.limit.unwrap() <= 50 && page.cursor.is_none());
        if self.fault == "page-error" {
            return Err(MetricsError::Busy);
        }
        if self.fault == "expires" {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        let measurement =
            |metric| Measurement::project(&Aggregate::default(), metric, AT, true, 0.);
        let mut rows = Rows {
            items: vec![Row {
                sandbox_id: SANDBOX.into(),
                name: "Sandbox".into(),
                owner: Owner {
                    subject: "owner".into(),
                    display_name: Some("Current public name".into()),
                },
                status: "active".into(),
                runtime_version: None,
                agent_count: 1,
                last_activity_at: None,
                allocation: Capacity::default(),
                allocated_period: AllocatedPeriod::default(),
                measured: Measured {
                    cpu: measurement(Metric::Cpu),
                    memory: measurement(Metric::Memory),
                    disk: measurement(Metric::Disk),
                },
                billing: Billing::default(),
                shared_sandbox_usage: false,
            }],
            next: None,
            report_id: report.report_id.clone(),
            as_of: AT,
            total_count: 1,
        };
        match self.fault {
            "rows-report" => rows.report_id = "foreign".into(),
            "rows-as-of" => rows.as_of += 1,
            "rows-count" => rows.total_count = 2,
            "rows-missing" => rows.items.clear(),
            "rows-extra" => rows.items.push(rows.items[0].clone()),
            "rows-owner" => rows.items[0].owner.subject = "foreign".into(),
            _ => {}
        }
        Ok(rows)
    }
    async fn detail(&self, _: &Report, _: &str) -> Result<Row> {
        panic!("no arbitrary child selection")
    }
    async fn series(&self, report: &Report, query: SeriesQuery) -> Result<Series> {
        self.call("series");
        assert_eq!(report.report_id, self.report.report_id);
        assert_eq!(query.group_by, GroupBy::Total);
        assert!(query.series_id.is_none() && query.top.is_none());
        if self.fault == "series-error" && query.metric == Metric::Memory {
            return Err(MetricsError::Busy);
        }
        let mut series = Series {
            report_id: report.report_id.clone(),
            metric: query.metric,
            unit: query.metric.unit().into(),
            source: SOURCE.into(),
            lines: vec![Line {
                id: "total".into(),
                label: "Total".into(),
                points: series::buckets(
                    &[],
                    query.metric,
                    report.effective_range,
                    report.query.step_seconds,
                )
                .into_iter()
                .map(series::Bucket::point)
                .collect(),
            }],
        };
        match self.fault {
            "series-report" => series.report_id = "foreign".into(),
            "series-metric" => series.metric = Metric::Disk,
            "series-unit" => series.unit = "incorrect".into(),
            "series-group" => series.lines[0].id = "owner".into(),
            "series-points" => {
                let point = series.lines[0].points[0].clone();
                series.lines[0].points.resize(2017, point);
            }
            "series-range" => series.lines[0].points[0].from += 1,
            _ => {}
        }
        Ok(series)
    }
}
