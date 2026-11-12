//! Flush even when no commands finish, so an active stall is observable.
use super::{micros, summary, Diagnostics};
use std::{sync::Arc, time::Duration};
use tokio::{task::JoinHandle, time::MissedTickBehavior};

pub(super) fn start(diagnostics: &Diagnostics) -> Option<JoinHandle<()>> {
    let runtime = tokio::runtime::Handle::try_current().ok()?;
    let weak = Arc::downgrade(&diagnostics.0);
    let pool = diagnostics.1;
    Some(runtime.spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let Some(shared) = weak.upgrade() else {
                break;
            };
            let (completed, commands, holds, oldest_hold_us, untracked) = {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                let oldest = state
                    .holds
                    .values()
                    .map(|hold| micros(hold.start.elapsed()))
                    .max()
                    .unwrap_or(0);
                let untracked = state.summary.untracked;
                (
                    state.summary.drain(),
                    state.commands.len(),
                    state.holds.len(),
                    oldest,
                    untracked,
                )
            };
            drop(shared);
            // Never hold an observer mutex while formatting or writing logs.
            summary::emit(Some(completed));
            tracing::info!(target: "mongo_diagnostic_pool", parent: None,
                pool, tracked_commands = commands, tracked_holds = holds,
                oldest_tracked_hold_us = oldest_hold_us, untracked,
                "MongoDB diagnostic heartbeat");
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;
    #[tokio::test(start_paused = true)]
    async fn periodic_flush_does_not_need_another_completion_and_stops_with_client() {
        let output = super::super::tests::Output::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(output.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let diagnostics = Diagnostics::default();
        let task = start(&diagnostics).unwrap();
        let command = super::super::metadata("find", &doc! {"find":"agents"}, false);
        diagnostics.0.lock().unwrap().summary.record(
            &command,
            "command",
            Duration::from_millis(2100),
            Some(1),
            false,
        );
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(10)).await;
        tokio::task::yield_now().await;
        let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert!(logs.contains("MongoDB attributed totals"));
        assert!(logs.contains("total_us=2100000"));
        assert!(logs.contains("collection=\"agents\""));
        assert!(logs.contains("MongoDB diagnostic heartbeat"));
        assert!(logs.contains("tracked_commands=0"));
        drop(diagnostics);
        tokio::time::advance(Duration::from_secs(10)).await;
        task.await.unwrap();
    }
}
