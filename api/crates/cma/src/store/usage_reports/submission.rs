//! Notifications accelerate local work; Mongo leases and records stay authoritative.
use super::*;
use layer2_core::sandbox_metrics::{MetricsStore, Report, ReportQuery, ReportStatus, Scope};
use tokio::sync::{broadcast, watch};

pub(super) struct ReportNotifications {
    work: watch::Sender<()>,
    finished: broadcast::Sender<String>,
}

impl Default for ReportNotifications {
    fn default() -> Self {
        Self {
            work: watch::channel(()).0,
            finished: broadcast::channel(16).0,
        }
    }
}

impl ReportNotifications {
    pub(super) fn wake(&self) {
        self.work.send_replace(());
    }

    pub(super) fn finished(&self, id: &str) {
        // Completion is durable before notification. No listeners is normal
        // when the requesting client disconnected or already received a 202.
        if self.finished.send(id.into()).is_err() {
            tracing::debug!(
                report_id = id,
                "sandbox report finished without local waiters"
            );
        }
    }
}

impl ReportingStore {
    pub(crate) fn report_work(&self) -> watch::Receiver<()> {
        self.notifications.work.subscribe()
    }

    pub(super) async fn submit_with_completion(
        &self,
        scope: Scope,
        query: ReportQuery,
        at: u64,
    ) -> Result<Report> {
        let started = tokio::time::Instant::now();
        let deadline = started + Duration::from_secs(1);
        // Subscribe before admission so even a very fast worker cannot finish
        // between submitting the job and registering its waiter.
        let mut finished = self.notifications.finished.subscribe();
        for attempt in 0..3 {
            let report = match self.admit(scope.clone(), query.clone(), at).await {
                // Give the winning admission time to commit. Merely yielding
                // can exhaust all three attempts before Mongo resolves the
                // conflict, preventing concurrent requests from coalescing.
                Err(MetricsError::Unavailable) if attempt < 2 => {
                    tokio::time::sleep(Duration::from_millis(25 << attempt)).await;
                    continue;
                }
                result => result?,
            };
            let report = if report.status == ReportStatus::Pending
                && self.notifications.work.receiver_count() > 0
            {
                let completion = async {
                    // A worker on another replica cannot notify this process.
                    // Make one durable status read near the response deadline,
                    // reserving its final 250 ms for the isolated Mongo pool.
                    let probe_at = deadline - Duration::from_millis(250);
                    loop {
                        let event = tokio::select! {
                            event = finished.recv() => event,
                            _ = tokio::time::sleep_until(probe_at) => break,
                        };
                        match event {
                            Ok(id) if id != report.report_id => continue,
                            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => break,
                            Err(broadcast::error::RecvError::Closed) => {
                                return Err(MetricsError::Unavailable);
                            }
                        }
                    }
                    // Notifications contain no report data and never bypass
                    // scope lookup or the service's ownership reauthorization.
                    self.check_circuit(now())?;
                    self.report(&scope, &report.report_id, now()).await
                };
                match tokio::time::timeout_at(deadline, completion).await {
                    Ok(result) => result?,
                    // No DB connection or transaction is held while waiting.
                    // Slow jobs retain the usual poll path.
                    Err(_) => report,
                }
            } else {
                report
            };
            tracing::info!(
                report_id = %report.report_id,
                status = ?report.status,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "sandbox report submission completed"
            );
            return Ok(report);
        }
        Err(MetricsError::Unavailable)
    }
}
