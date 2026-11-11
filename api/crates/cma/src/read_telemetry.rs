//! Periodic content-free read diagnostics on the existing structured log path.
use std::time::Duration;
use tokio::{sync::watch, task::JoinHandle, time::MissedTickBehavior};

pub(crate) fn spawn(signal: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let stopped = crate::shutdown::signalled(signal);
        tokio::pin!(stopped);
        loop {
            tokio::select! {
                biased;
                () = &mut stopped => break,
                _ = interval.tick() => emit(),
            }
        }
    })
}

pub(crate) fn emit() {
    layer1_public::stream_telemetry::emit();
    crate::store::agent_log_telemetry::emit();
    crate::store::mongo_read_telemetry::emit();
    crate::store::emit_response_reads();
}
