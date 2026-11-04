//! Real metrics router/service contracts; only persistence/source reads are fake.
use super::Scenario;
use async_trait::async_trait;
use layer2_core::{sandbox_metrics::*, Authorization, Capability};
use std::sync::Arc;
const ID: &str = "umr_00000000000000000000000000000001";
const SANDBOX: &str = "sbx_00000000000000000000000000000001";
struct Store {
    mode: String,
}
impl Store {
    fn check(&self) -> Result<()> {
        match self.mode.as_str() {
            "busy" => Err(MetricsError::Busy),
            "unavailable" => Err(MetricsError::Unavailable),
            "missing" => Err(MetricsError::NotFound),
            "invalid" => Err(MetricsError::Invalid("Invalid retained cursor".into())),
            _ => Ok(()),
        }
    }
    fn report(&self, scope: Scope) -> Report {
        let query = ReportInput {
            from: Some(1000),
            to: Some(1060),
            ..Default::default()
        }
        .validate(1180)
        .unwrap();
        Report {
            schema_version: 1,
            report_id: ID.into(),
            status: if self.mode == "pending" {
                ReportStatus::Pending
            } else {
                ReportStatus::Ready
            },
            report_complete: self.mode != "pending",
            scope: scope.clone(),
            query,
            as_of: 1180,
            measurement_cutoff: 1060,
            effective_range: Range {
                from: 1000,
                to: 1060,
            },
            expires_at: if self.mode == "expired" { 0 } else { u64::MAX },
            allocation_watermark: "fixture:1".into(),
            mapping_revision: 1,
            available_from: Some(1000),
            tracking_started_at: Some(1000),
            warnings: vec!["measurement_coverage_incomplete".into()],
            source_health: "partial".into(),
            summary: Some(Summary {
                sandbox_count: 1,
                running_count: 1,
                measured: Some(row().measured),
                infrastructure: scope.admin.then(|| Infrastructure {
                    state: "partial".into(),
                    unproven_count: 1,
                    unassigned_pool: [Some(0.2), Some(1024.), None],
                    unattributed: [Some(0.1), None, Some(2048.)],
                    observed_at: Some(1040),
                }),
                ..Default::default()
            }),
            poll_url: format!(
                "/api/v1/{}usage/metrics/reports/{ID}",
                if scope.admin { "admin/" } else { "" }
            ),
            retry_after_seconds: 2,
        }
    }
}
fn row() -> Row {
    let cpu = Aggregate {
        integral: 30.,
        display_integral: 30.,
        covered_seconds: 30.,
        expected_seconds: 60.,
        min: Some(0.5),
        max: Some(1.),
        latest: Some(1.),
        observed_at: Some(1040.),
        ..Default::default()
    };
    let memory = Aggregate {
        integral: 30. * 1024.,
        display_integral: 30. * 1024.,
        covered_seconds: 30.,
        expected_seconds: 60.,
        latest: Some(1024.),
        observed_at: Some(1040.),
        ..Default::default()
    };
    let disk = Aggregate {
        integral: 30. * 2048.,
        display_integral: 30. * 2048.,
        covered_seconds: 30.,
        expected_seconds: 60.,
        latest: Some(2048.),
        observed_at: Some(1040.),
        ..Default::default()
    };
    Row {
        sandbox_id: SANDBOX.into(),
        name: "Shared measured Sandbox".into(),
        owner: Owner {
            subject: "owner".into(),
            display_name: None,
        },
        status: "active".into(),
        runtime_version: Some("v1".into()),
        agent_count: u64::MAX,
        last_activity_at: Some(1040),
        allocation: Capacity {
            cpu_cores: 2.,
            memory_bytes: 9_007_199_254_740_992.,
            disk_bytes: 4096.,
        },
        allocated_period: AllocatedPeriod::default(),
        measured: Measured {
            cpu: Measurement::project(&cpu, Metric::Cpu, 1060, true, 0.),
            memory: Measurement::project(&memory, Metric::Memory, 1060, true, 0.),
            disk: Measurement::project(&disk, Metric::Disk, 1180, false, 0.),
        },
        billing: Billing::default(),
        shared_sandbox_usage: true,
    }
}
#[async_trait]
impl MetricsStore for Store {
    async fn resolve_agent(&self, s: &Scope, _: &str) -> Result<String> {
        self.authorize_sandbox(s, SANDBOX).await?;
        Ok(SANDBOX.into())
    }
    async fn authorize_sandbox(&self, s: &Scope, id: &str) -> Result<()> {
        if id != SANDBOX || (!s.admin && s.principal != "owner") {
            Err(MetricsError::NotFound)
        } else {
            self.check()
        }
    }
    async fn submit(&self, s: Scope, _: ReportQuery, _: u64) -> Result<Report> {
        self.check()?;
        Ok(self.report(s))
    }
    async fn report(&self, s: &Scope, id: &str, _: u64) -> Result<Report> {
        self.check()?;
        if id != ID {
            return Err(MetricsError::NotFound);
        }
        Ok(self.report(s.clone()))
    }
    async fn reauthorize(&self, _: &Report) -> Result<()> {
        self.check()
    }
    async fn page(&self, r: &Report, _: PageQuery) -> Result<Rows> {
        Ok(Rows {
            items: vec![row()],
            next: None,
            report_id: r.report_id.clone(),
            as_of: r.as_of,
            total_count: 1,
        })
    }
    async fn detail(&self, _: &Report, _: &str) -> Result<Row> {
        Ok(row())
    }
    async fn series(&self, r: &Report, q: SeriesQuery) -> Result<Series> {
        Ok(Series {
            report_id: r.report_id.clone(),
            metric: q.metric,
            unit: q.metric.unit().into(),
            source: SOURCE.into(),
            lines: vec![Line {
                id: "total".into(),
                label: "Total".into(),
                points: vec![Point {
                    from: 1000,
                    to: 1060,
                    mean: None,
                    min: None,
                    max: None,
                    covered_seconds: 0.,
                    expected_seconds: 60.,
                    unknown_mapping_seconds: 30.,
                    numerator: None,
                    denominator: None,
                    reason: Some("unattributed".into()),
                    excluded_count: 1,
                }],
            }],
        })
    }
    async fn owners(&self, r: &Report, _: OwnerQuery) -> Result<Owners> {
        Ok(Owners {
            items: vec![row().owner],
            next: None,
            report_id: r.report_id.clone(),
        })
    }
}
pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let tail = name.strip_prefix("metrics_")?;
    let admin = tail.starts_with("admin_");
    let mode = tail.strip_prefix("admin_").unwrap_or(tail);
    let subject = if mode == "anonymous" {
        None
    } else {
        Some(if admin { "operator" } else { "owner" })
    };
    let capabilities = if mode == "forbidden" {
        vec![]
    } else if admin {
        vec![Capability::ReadAllSandboxes]
    } else {
        vec![Capability::UseOwnSandboxes]
    };
    Some(Scenario {
        router: super::super::sandbox_metrics_router(MetricsService::new(Arc::new(Store {
            mode: mode.into(),
        }))),
        subject,
        authorization: Authorization::new(capabilities),
        symbols: Default::default(),
    })
}
