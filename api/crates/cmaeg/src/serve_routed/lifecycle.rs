//! Supervised recovery owned by the HTTP service's lifetime and drain deadline.
use crate::{
    http::readiness::Readiness,
    turns::routed::scheduler::{ScheduleReport, Scheduler},
};
use futures_util::FutureExt;
use std::{panic::AssertUnwindSafe, sync::Arc, time::Instant};
use tokio::{sync::watch, task::JoinHandle};

pub(crate) struct Worker {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<ScheduleReport>>,
}
impl Worker {
    pub fn start(scheduler: Scheduler, readiness: Readiness) -> Self {
        let (stop, stopped) = watch::channel(false);
        let signal = stopped.clone();
        let failure = readiness.clone();
        let scheduler = scheduler.with_failure_reporter(Arc::new(move || {
            failure.routed_recovery_failed();
        }));
        let task = tokio::spawn(async move {
            let report = match AssertUnwindSafe(scheduler.run(stopped))
                .catch_unwind()
                .await
            {
                Ok(report) => report,
                Err(_) => ScheduleReport {
                    recovery_failed: true,
                    ..Default::default()
                },
            };
            if report.recovery_failed || !*signal.borrow() {
                readiness.routed_recovery_failed();
                tracing::error!("routed execution worker stopped unexpectedly");
            }
            report
        });
        Self {
            stop,
            task: Some(task),
        }
    }

    pub fn stop_scheduling(&self) {
        let _ = self.stop.send(true);
    }

    pub async fn drain(&mut self, deadline: Instant) -> bool {
        self.stop_scheduling();
        let Some(mut task) = self.task.take() else {
            return true;
        };
        match tokio::time::timeout_at(deadline.into(), &mut task).await {
            Ok(Ok(report)) => report.aborted == 0 && !report.recovery_failed,
            Ok(Err(_)) => false,
            Err(_) => {
                task.abort();
                let _ = task.await;
                false
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop_scheduling();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
