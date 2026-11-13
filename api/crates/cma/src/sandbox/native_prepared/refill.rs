//! Local consumption wakes refill; periodic discovery still recovers lost hints.
use tokio::sync::Notify;
use tokio::time::Interval;

pub(super) async fn changed(ticker: &mut Interval, consumed: &Notify) {
    tokio::select! {
        _ = ticker.tick() => {},
        _ = consumed.notified() => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::time::Duration;
    use tokio::time::{Instant, MissedTickBehavior};

    #[tokio::test(start_paused = true)]
    async fn consumption_refills_before_recovery_tick_and_coalesces() {
        let now = Instant::now();
        let mut ticker =
            tokio::time::interval_at(now + Duration::from_secs(2), Duration::from_secs(2));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let consumed = Notify::new();
        assert!(changed(&mut ticker, &consumed).now_or_never().is_none());
        consumed.notify_one();
        consumed.notify_one();
        changed(&mut ticker, &consumed).await;
        assert_eq!(Instant::now(), now);
        assert!(changed(&mut ticker, &consumed).now_or_never().is_none());
        changed(&mut ticker, &consumed).await;
        assert_eq!(Instant::now() - now, Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_worker_observes_consumption_without_losing_timer_fallback() {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        let consumed = Notify::new();
        // Startup discovery remains immediate even before any admission.
        changed(&mut ticker, &consumed).await;
        let now = Instant::now();
        let wait = changed(&mut ticker, &consumed);
        tokio::pin!(wait);
        assert!(wait.as_mut().now_or_never().is_none());
        consumed.notify_one();
        wait.await;
        assert_eq!(Instant::now(), now);
    }
}
