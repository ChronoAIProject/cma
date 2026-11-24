use super::*;
use crate::rest::sandbox_metrics::tests as legacy;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct Store {
    pub base: legacy::Store,
    pub auth: AtomicUsize,
    pub admissions: AtomicUsize,
    pub pages: AtomicUsize,
    pub charts: AtomicUsize,
    pub members: u64,
    pub fault: &'static str,
}
impl Store {
    pub fn new(fault: &'static str, members: u64) -> Arc<Self> {
        Arc::new(Self {
            base: legacy::Store::default(),
            auth: AtomicUsize::new(0),
            admissions: AtomicUsize::new(0),
            pages: AtomicUsize::new(0),
            charts: AtomicUsize::new(0),
            members,
            fault,
        })
    }
}
#[async_trait]
impl MetricsStore for Store {
    async fn initial_snapshot_generation(&self, _: &Report) -> Result<u64> {
        Ok(1)
    }
    async fn resolve_agent(&self, scope: &Scope, id: &str) -> Result<String> {
        self.base.resolve_agent(scope, id).await
    }
    async fn authorize_sandbox(&self, scope: &Scope, id: &str) -> Result<()> {
        self.base.authorize_sandbox(scope, id).await
    }
    async fn submit(&self, scope: Scope, query: ReportQuery, at: u64) -> Result<Report> {
        self.admissions.fetch_add(1, Ordering::SeqCst);
        let mut report = self.base.submit(scope, query, at).await?;
        report.summary.as_mut().unwrap().sandbox_count = self.members;
        if self.fault == "pending" {
            report.status = ReportStatus::Pending;
            report.report_complete = false;
            report.summary = None;
        }
        *self.base.saved.lock().unwrap() = Some(report.clone());
        Ok(report)
    }
    async fn latest(
        &self,
        scope: &Scope,
        selection: &ViewSelection,
        at: u64,
    ) -> Result<ReportView> {
        if self.fault == "disabled" {
            return Err(MetricsError::ViewDisabled);
        }
        let report = self
            .submit(scope.clone(), selection.query.clone(), at)
            .await?;
        let pending = report.status == ReportStatus::Pending;
        Ok(ReportView {
            schema_version: 1,
            window: selection.window,
            requested_range: selection.query.range(),
            generated_at: (!pending).then_some(report.as_of),
            stale: self.fault == "stale",
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
        })
    }
    async fn report(&self, scope: &Scope, id: &str, at: u64) -> Result<Report> {
        self.base.report(scope, id, at).await
    }
    async fn reauthorize(&self, report: &Report) -> Result<()> {
        self.auth.fetch_add(1, Ordering::SeqCst);
        self.base.reauthorize(report).await
    }
    async fn page(&self, report: &Report, query: PageQuery) -> Result<Rows> {
        self.pages.fetch_add(1, Ordering::SeqCst);
        if self.fault == "slow" {
            tokio::time::sleep(std::time::Duration::from_secs(6)).await;
        }
        let mut rows = self.base.page(report, query.clone()).await?;
        rows.total_count = self.members;
        rows.items = (0..self.members.min(u64::from(query.limit.unwrap())))
            .map(|i| {
                let measurement = |metric| {
                    Measurement::project(&Aggregate::default(), metric, report.as_of, true, 0.)
                };
                Row {
                    sandbox_id: format!("sbx_{:032x}", i + 1),
                    name: if self.fault == "oversize" {
                        "x".repeat(encoding::MAX_INITIAL_BYTES)
                    } else {
                        if self.fault == "wide" {
                            "\"".repeat(512)
                        } else {
                            "Fixture Sandbox".into()
                        }
                    },
                    owner: Owner {
                        subject: report.scope.principal.clone(),
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
                }
            })
            .collect();
        rows.next = (self.members > rows.items.len() as u64).then(|| "signed-next".into());
        Ok(rows)
    }
    async fn detail(&self, report: &Report, id: &str) -> Result<Row> {
        self.base.detail(report, id).await
    }
    async fn series(&self, report: &Report, query: SeriesQuery) -> Result<Series> {
        self.charts.fetch_add(1, Ordering::SeqCst);
        if self.fault == "child-error" && query.metric == Metric::Memory {
            return Err(MetricsError::Unavailable);
        }
        let mut result = self.base.series(report, query.clone()).await?;
        if self.members > 0 {
            result.lines.push(Line {
                id: "total".into(),
                label: "Total".into(),
                points: layer2_core::sandbox_metrics::series::buckets(
                    &[],
                    query.metric,
                    report.effective_range,
                    report.query.step_seconds,
                )
                .into_iter()
                .map(layer2_core::sandbox_metrics::series::Bucket::point)
                .collect(),
            });
        }
        if self.fault == "wide" {
            for point in result.lines.iter_mut().flat_map(|line| &mut line.points) {
                let wide = 9_007_199_254_740_992.;
                point.mean = Some(wide);
                point.min = Some(wide);
                point.max = Some(wide);
                point.numerator = Some(wide);
                point.denominator = Some(wide);
                point.covered_seconds = wide;
                point.expected_seconds = wide;
                point.unknown_mapping_seconds = wide;
                point.excluded_count = u64::MAX;
            }
        }
        Ok(result)
    }
}
