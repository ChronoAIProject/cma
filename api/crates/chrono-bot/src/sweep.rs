//! Keeping what is running in line with what is recorded.
//!
//! Two cadences, because the two jobs answer to different clocks.
//!
//! Reconciliation is slow. Everything it acts on is a disagreement between the
//! record and the provider, and those are rare, so asking often would query the
//! provider constantly for something almost always empty.
//!
//! Capturing transcripts is quick. What it protects against is this process
//! dying, which is not something it will be warned about, and the interval is
//! therefore the amount of a session's history a crash is allowed to cost.
//!
//! The work itself belongs to the core. This only decides when it happens.

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

/// How often to store what live sessions have shown.
///
/// This is the window of history a sudden death can lose, so it is short. It is
/// also cheap: a session with no new output writes nothing at all.
const CAPTURE_INTERVAL: Duration = Duration::from_secs(15);

/// Delay before the first sweep.
///
/// A restarted process has in-flight work to re-establish, and sweeping
/// immediately would judge sessions before anything has had a chance to
/// reclaim them.
const SETTLE_DELAY: Duration = Duration::from_secs(60);

/// Start both loops in the background for as long as the process runs.
pub fn spawn(service: Arc<SessionService>) {
    // Once, immediately, before anything is served. A process that has just
    // started is the case this exists for: every session it should be running
    // is one it has never heard of, and a request arriving first would find a
    // session that looks alive and answers nothing.
    let boot = Arc::clone(&service);
    tokio::spawn(async move {
        let recovered = boot.reconcile().await;
        tracing::info!(
            readopted = recovered.readopted_sessions,
            reprovisioned = recovered.reprovisioned_sessions,
            reclaimed = recovered.orphaned_runtimes,
            "recovered recorded sessions on startup"
        );
    });

    let reconciler = Arc::clone(&service);
    tokio::spawn(async move {
        tokio::time::sleep(SETTLE_DELAY).await;
        tracing::info!(
            sweep_seconds = SWEEP_INTERVAL.as_secs(),
            "reconciliation started"
        );
        loop {
            reconciler.reconcile().await;
            // Sessions recovered by the sweep have no credential of their own
            // yet, and one recovered before its owner signed back in cannot be
            // given one at the time.
            reconciler.top_up_credentials().await;
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    });

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CAPTURE_INTERVAL);
        loop {
            ticker.tick().await;
            service.capture_transcripts().await;
        }
    });
    tracing::info!(
        capture_seconds = CAPTURE_INTERVAL.as_secs(),
        "transcript capture started"
    );
}
