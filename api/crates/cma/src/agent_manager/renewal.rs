//! Renewal deadlines progress independently of retirement and receiver IO.
use super::*;
use layer2_core::agent_ownership::{Authority, OwnershipError, HEARTBEAT_INTERVAL_MS};
use std::future::Future;

#[derive(Debug)]
struct RenewalFailure {
    phase: &'static str,
    error: OwnershipError,
}

pub(super) fn spawn(manager: &Arc<AgentManager>) {
    let weak = Arc::downgrade(manager);
    let coordinator = manager.coordinator.clone();
    // Start during composition: dependency startup may exceed a lease.
    tokio::spawn(async move {
        let result = run(coordinator.clone(), move |candidates| {
            let weak = weak.clone();
            async move {
                let manager = weak.upgrade().ok_or(OwnershipError::Stale)?;
                manager.maintain_receivers(candidates).await
            }
        })
        .await;
        finish(&coordinator, result).await;
    });
}

async fn finish(coordinator: &OwnershipCoordinator, result: Result<(), RenewalFailure>) {
    if let Err(failure) = result {
        // maintain() can wake failed() before returning its error. Shutdown may
        // already have begun draining by the time this result is handled.
        let during_drain = coordinator.is_draining();
        let error_kind = match &failure.error {
            OwnershipError::Stale => "stale",
            OwnershipError::IncompatibleProtocol => "incompatible_protocol",
            OwnershipError::Invalid(_) => "invalid",
            OwnershipError::Unavailable(_) => "unavailable",
        };
        tracing::error!(
            phase = failure.phase,
            error_kind,
            error = %failure.error,
            during_drain,
            "Agent Manager lost membership or ownership; restarting is required"
        );
        if !during_drain {
            coordinator.fail().await;
        }
    }
}

async fn run<F, Work>(
    coordinator: Arc<OwnershipCoordinator>,
    mut work: F,
) -> Result<(), RenewalFailure>
where
    F: FnMut(Vec<Arc<Authority>>) -> Work,
    Work: Future<Output = Result<(), OwnershipError>>,
{
    // Retain only the newest bounded candidate set while housekeeping is busy.
    // No receiver operation runs on the renewal future's critical path.
    let (latest, mut changes) = tokio::sync::watch::channel(Vec::new());
    let renewing = async {
        let period = Duration::from_millis(HEARTBEAT_INTERVAL_MS);
        let mut ticks = tokio::time::interval_at(Instant::now() + period, period);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            if coordinator.is_draining() {
                return Ok(());
            }
            let candidates = coordinator
                .maintain()
                .await
                .map_err(|error| RenewalFailure {
                    phase: "ownership_maintenance",
                    error,
                })?;
            latest.send_replace(candidates);
        }
    };
    let housekeeping = async {
        while changes.changed().await.is_ok() {
            let candidates = changes.borrow_and_update().clone();
            work(candidates).await.map_err(|error| RenewalFailure {
                phase: "receiver_housekeeping",
                error,
            })?;
        }
        Ok(())
    };
    tokio::select! {
        biased;
        result = renewing => result,
        result = housekeeping => result,
    }
}

#[cfg(test)]
mod tests;
