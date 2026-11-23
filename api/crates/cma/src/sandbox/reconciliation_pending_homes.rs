//! Every replica checks one bounded receipt page about once every five minutes.
use super::SandboxReconciler;
use crate::store::pending_launch_homes::{PendingLaunchHomes, SweepReport};
use layer2_core::StoreError;
use mongodb::bson::Document;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

#[async_trait::async_trait]
pub(crate) trait PendingHomesSweep: Send + Sync {
    async fn sweep_pending_homes(
        &self,
        after: Option<Document>,
        progress: &std::sync::Mutex<Option<Document>>,
    ) -> Result<SweepReport, StoreError>;
}
#[async_trait::async_trait]
impl PendingHomesSweep for PendingLaunchHomes {
    async fn sweep_pending_homes(
        &self,
        after: Option<Document>,
        progress: &std::sync::Mutex<Option<Document>>,
    ) -> Result<SweepReport, StoreError> {
        self.sweep_with_progress(after, Some(progress)).await
    }
}

const INTERVAL: Duration = Duration::from_secs(300);
const OPPORTUNITY: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct PendingHomesSchedule {
    last: Option<SystemTime>,
    after: std::sync::Mutex<Option<Document>>,
}
impl PendingHomesSchedule {
    async fn run(&mut self, sweep: &dyn PendingHomesSweep, now: SystemTime) {
        if self.last.is_some_and(|at| {
            now.duration_since(at)
                .is_ok_and(|elapsed| elapsed < INTERVAL)
        }) {
            return;
        }
        // Reserve the interval before I/O, including cancellation or failure.
        self.last = Some(now);
        let after = self
            .after
            .lock()
            .expect("pending home cursor poisoned")
            .clone();
        match tokio::time::timeout(OPPORTUNITY, sweep.sweep_pending_homes(after, &self.after)).await
        {
            Ok(Ok(report)) => {
                *self.after.lock().expect("pending home cursor poisoned") = report.next;
                tracing::info!(target: "cma::admission", event = "pending_launch_homes_sweep",
                    scanned = report.scanned, released = report.released,
                    missing_launches = report.missing_launches,
                    "pending Launch home reconciliation page completed");
            }
            Ok(Err(error)) => {
                tracing::warn!(target: "cma::admission", event = "pending_launch_homes_sweep_failed",
                error = %error, "pending Launch home reconciliation is unavailable")
            }
            Err(_) => {
                tracing::warn!(target: "cma::admission", event = "pending_launch_homes_sweep_failed",
                reason = "deadline", "pending Launch home reconciliation opportunity expired")
            }
        }
    }
}

impl SandboxReconciler {
    pub(crate) fn with_pending_homes(mut self, sweep: Arc<dyn PendingHomesSweep>) -> Self {
        self.pending_homes = Some(sweep);
        self
    }
    pub(super) async fn reconcile_pending_homes(&self, now: SystemTime) {
        if let Some(sweep) = &self.pending_homes {
            self.pending_homes_schedule
                .lock()
                .await
                .run(sweep.as_ref(), now)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Default)]
    struct Sweep(AtomicUsize);
    #[async_trait::async_trait]
    impl PendingHomesSweep for Sweep {
        async fn sweep_pending_homes(
            &self,
            _: Option<Document>,
            _: &std::sync::Mutex<Option<Document>>,
        ) -> Result<SweepReport, StoreError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(StoreError::Unavailable("unreadable capacity state".into()))
        }
    }
    #[tokio::test]
    async fn pending_home_failures_are_reported_and_throttled_on_each_replica() {
        let sweep = Sweep::default();
        let mut first = PendingHomesSchedule::default();
        let mut second = PendingHomesSchedule::default();
        for seconds in [0, 1, 299, 300] {
            first
                .run(
                    &sweep,
                    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
                )
                .await;
        }
        second.run(&sweep, SystemTime::UNIX_EPOCH).await;
        assert_eq!(sweep.0.load(Ordering::Relaxed), 3);
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use mongodb::bson::doc;
    struct Slow;
    #[async_trait::async_trait]
    impl PendingHomesSweep for Slow {
        async fn sweep_pending_homes(
            &self,
            after: Option<Document>,
            progress: &std::sync::Mutex<Option<Document>>,
        ) -> Result<SweepReport, StoreError> {
            if let Some(after) = after {
                assert_eq!(after.get_str("launch").unwrap(), "slow-prefix");
                Ok(SweepReport {
                    scanned: 1,
                    released: 1,
                    ..Default::default()
                })
            } else {
                *progress.lock().unwrap() =
                    Some(doc! {"scope":"launch_home_capacity","launch":"slow-prefix"});
                std::future::pending().await
            }
        }
    }
    #[tokio::test(start_paused = true)]
    async fn cancellation_retains_candidate_progress_and_a_later_page_can_repair() {
        let mut schedule = PendingHomesSchedule::default();
        schedule.run(&Slow, SystemTime::UNIX_EPOCH).await;
        assert!(schedule.after.lock().unwrap().is_some());
        schedule.run(&Slow, SystemTime::UNIX_EPOCH + INTERVAL).await;
        assert!(schedule.after.lock().unwrap().is_none());
    }
}
