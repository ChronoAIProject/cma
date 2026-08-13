//! Periodic reconciliation between what is running and what is recorded.
//!
//! Sessions no longer expire on a timer, so nothing else in this service
//! reclaims anything. That makes this loop the only reaper there is, and its
//! absence would show up as sandboxes accumulating quietly until something
//! runs out.
//!
//! The work itself belongs to the core, which owns the comparison. This only
//! decides when it happens.

use std::sync::Arc;
use std::time::Duration;

use layer2_core::SessionService;

/// How often to compare the provider's runtimes against the record.
///
/// Unhurried on purpose. Everything it finds is already inert — a runtime
/// nobody can reach, or a session nothing is starting — so reclaiming it a few
/// minutes later costs only the resources it holds in the meantime, while a
/// tight loop would query the provider constantly for something that is
/// almost always empty.
const SWEEP_INTERVAL: Duration = Duration::from_secs(300);

/// Delay before the first sweep.
///
/// A restarted process has in-flight work to re-establish, and sweeping
/// immediately would judge sessions before anything has had a chance to
/// reclaim them.
const SETTLE_DELAY: Duration = Duration::from_secs(60);

/// Start reconciling in the background for as long as the process runs.
pub fn spawn(service: Arc<SessionService>) {
    tokio::spawn(async move {
        tokio::time::sleep(SETTLE_DELAY).await;
        tracing::info!(
            sweep_seconds = SWEEP_INTERVAL.as_secs(),
            "reconciliation started"
        );
        loop {
            service.reconcile().await;
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    });
}
