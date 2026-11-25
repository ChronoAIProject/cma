//! Bounded wake-ups over durable admissions; queue contents are never authority.
use crate::ingress::routing_store::{AdmittedPlan, EventIdentity, PlanStore, RecoveryPage};
use crate::replicas::diagnostics::{LogLimit, ScanError, Stage};
use cmaeg_engine::WorkSelection;
pub use cmaeg_engine::{Disposition, ScheduleConfig, ScheduleReport};
use cmaeg_execution::turns::routed_scheduler::stopped;
use futures_util::future::BoxFuture;
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Executors must use the persisted target and fenced journal before effects.
/// Returning Retry never authorizes another provider POST or platform send.
pub trait TargetExecutor: Send + Sync {
    fn execute<'a>(&'a self, work: &'a WorkItem) -> BoxFuture<'a, Disposition>;
}

pub trait RecoverySource: Send + Sync {
    fn start_measurement(&self) -> Option<crate::telemetry::provider_start::Measurement> {
        None
    }
    fn page<'a>(&'a self, after: Option<&'a str>)
        -> BoxFuture<'a, Result<RecoveryPage, ScanError>>;
}
impl RecoverySource for PlanStore {
    fn start_measurement(&self) -> Option<crate::telemetry::provider_start::Measurement> {
        self.start_measurement.clone()
    }
    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RecoveryPage, ScanError>> {
        Box::pin(async move {
            self.recover_all_page(after, 32)
                .await
                .map_err(|e| ScanError::new(e, Stage::AdmissionScan, None))
        })
    }
}

#[derive(Clone)]
pub struct WorkItem {
    pub identity: EventIdentity,
    pub target_id: String,
}
impl WorkSelection for WorkItem {
    fn key(&self) -> (String, String) {
        (self.identity.admission_id(), self.target_id.clone())
    }
    fn serial_key(&self) -> (String, String) {
        (self.identity.record_id.to_string(), self.target_id.clone())
    }
}

#[test]
fn application_drain_fits_the_routed_scheduler_bound() {
    let config = ScheduleConfig {
        drain_timeout: crate::TurnConfig::default().drain,
        ..ScheduleConfig::default()
    };
    assert_eq!(config.drain_timeout, Duration::from_secs(660));
    assert!(config.valid());
    assert!(!ScheduleConfig {
        drain_timeout: Duration::from_secs(661),
        ..config
    }
    .valid());
}

#[derive(Clone)]
pub struct ScheduleHandle {
    sender: mpsc::Sender<WorkItem>,
}
impl ScheduleHandle {
    /// Closed on shutdown or loss of recovery; fresh admissions must pause.
    pub fn accepting(&self) -> bool {
        !self.sender.is_closed()
    }
    /// Only call after admission commits. Full/closed queues do not lose work:
    /// the durable scan will find the original manifest after restart or pressure.
    /// False means this local wake-up did not enqueue every target.
    pub fn notify(&self, plan: &AdmittedPlan) -> bool {
        let mut complete = true;
        for target in &plan.targets {
            if self
                .sender
                .try_send(WorkItem {
                    identity: plan.identity.clone(),
                    target_id: target.id.clone(),
                })
                .is_err()
            {
                complete = false;
            }
        }
        complete
    }
}

pub struct Scheduler {
    source: Arc<dyn RecoverySource>,
    scheduler: cmaeg_execution::turns::routed_scheduler::Scheduler<WorkItem>,
}

impl Scheduler {
    pub fn new(
        source: Arc<dyn RecoverySource>,
        executor: Arc<dyn TargetExecutor>,
        config: ScheduleConfig,
    ) -> Result<(Self, ScheduleHandle), &'static str> {
        let executor: Arc<dyn TargetExecutor> = match source.start_measurement() {
            Some(measurement) => Arc::new(super::measured_executor::MeasuredExecutor {
                executor,
                measurement,
            }),
            None => executor,
        };
        let execute = Arc::new(move |work: WorkItem| -> BoxFuture<'static, Disposition> {
            let executor = Arc::clone(&executor);
            Box::pin(async move { executor.execute(&work).await })
        });
        let (scheduler, sender) =
            cmaeg_execution::turns::routed_scheduler::Scheduler::new(execute, config)?;
        Ok((Self { source, scheduler }, ScheduleHandle { sender }))
    }

    pub fn with_failure_reporter(mut self, report: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.scheduler = self.scheduler.with_failure_reporter(report);
        self
    }

    pub async fn run(self, stop: watch::Receiver<bool>) -> ScheduleReport {
        let Self { source, scheduler } = self;
        scheduler
            .run(stop, move |sender, config, stop| {
                scan(source, sender, config, stop)
            })
            .await
    }
}

async fn scan(
    source: Arc<dyn RecoverySource>,
    sender: mpsc::Sender<WorkItem>,
    config: ScheduleConfig,
    mut stop: watch::Receiver<bool>,
) {
    let mut cursor = None;
    let mut logs = LogLimit::default();
    loop {
        if *stop.borrow() {
            return;
        }
        let page = tokio::select! {
            _ = stopped(&mut stop) => return,
            page = source.page(cursor.as_deref()) => page,
        };
        let delay = match page {
            Ok(page) => {
                logs.prune();
                for issue in &page.issues {
                    if issue.diagnostic.is_none() {
                        ScanError::new(issue.error.clone(), Stage::AdmissionScan, None)
                            .report(&mut logs);
                    }
                }
                for plan in page.plans {
                    for target in plan.targets {
                        let work = WorkItem {
                            identity: plan.identity.clone(),
                            target_id: target.id,
                        };
                        tokio::select! {
                            _ = stopped(&mut stop) => return,
                            sent = sender.send(work) => if sent.is_err() { return; },
                        }
                    }
                }
                cursor = page.next_cursor;
                if cursor.is_some() {
                    config.page_delay
                } else {
                    config.sweep_delay
                }
            }
            Err(error) => {
                // Preserve the current page after an outage. End-of-scan resets
                // guarantee inserts behind a prior cursor are discovered later.
                logs.prune();
                error.report_scan(&mut logs);
                config.sweep_delay
            }
        };
        tokio::select! {
            _ = stopped(&mut stop) => return,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}
