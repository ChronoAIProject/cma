//! Cadence for opportunistic shared-host recovery points.
//!
//! Eligibility, fencing, capture, verification, and publication belong to
//! Layer 2. This module decides only when to ask for one bounded pass.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::SessionService;
use tokio::time::MissedTickBehavior;

const CHECKPOINT_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Run checkpoint maintenance until the process exits.
pub(crate) fn spawn(service: Arc<SessionService>) {
    tokio::spawn(run_on_cadence(CHECKPOINT_SWEEP_INTERVAL, move || {
        let service = Arc::clone(&service);
        async move {
            let now = layer2_core::rfc3339_at(SystemTime::now());
            match service.checkpoint_due_chat_hosts(&now).await {
                Ok(sweep) if sweep.refused > 0 => tracing::warn!(
                    candidates = sweep.candidates,
                    completed = sweep.completed,
                    skipped = sweep.skipped,
                    refused = sweep.refused,
                    "chat checkpoint pass completed with refusals"
                ),
                Ok(sweep) if sweep.completed > 0 => tracing::info!(
                    candidates = sweep.candidates,
                    completed = sweep.completed,
                    skipped = sweep.skipped,
                    "chat checkpoint pass published recovery points"
                ),
                Ok(sweep) => tracing::debug!(
                    candidates = sweep.candidates,
                    skipped = sweep.skipped,
                    "chat checkpoint pass found no due quiet host"
                ),
                Err(error) => tracing::warn!(
                    %error,
                    "chat checkpoint pass could not read its durable candidates"
                ),
            }
        }
    }));
    tracing::info!(
        sweep_seconds = CHECKPOINT_SWEEP_INTERVAL.as_secs(),
        "chat checkpoint cadence started"
    );
}

async fn run_on_cadence<F, Fut>(interval: Duration, mut sweep: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        sweep().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn binary_cadence_is_one_minute_while_core_eligibility_is_five() {
        assert_eq!(CHECKPOINT_SWEEP_INTERVAL, Duration::from_secs(60));
        assert_eq!(layer2_core::CHAT_CHECKPOINT_INTERVAL_SECONDS, 300);
    }

    #[tokio::test(start_paused = true)]
    async fn cadence_ticks_immediately_then_once_per_minute() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let task = tokio::spawn(run_on_cadence(CHECKPOINT_SWEEP_INTERVAL, move || {
            observed.fetch_add(1, Ordering::SeqCst);
            std::future::ready(())
        }));
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        tokio::time::advance(Duration::from_secs(59)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn missed_ticks_are_skipped_instead_of_bursting() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let task = tokio::spawn(run_on_cadence(CHECKPOINT_SWEEP_INTERVAL, move || {
            observed.fetch_add(1, Ordering::SeqCst);
            std::future::ready(())
        }));
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        tokio::time::advance(Duration::from_secs(5 * 60)).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        task.abort();
    }
}
