//! Renewal deadlines progress independently of retirement and receiver IO.
use super::*;
use layer2_core::agent_ownership::{Authority, OwnershipError, HEARTBEAT_INTERVAL_MS};
use std::future::Future;

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
        if result.is_err() && !coordinator.is_draining() {
            coordinator.fail().await;
            tracing::error!("Agent Manager lost membership or ownership; restarting is required");
        }
    });
}

async fn run<F, Work>(
    coordinator: Arc<OwnershipCoordinator>,
    mut work: F,
) -> Result<(), OwnershipError>
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
            let candidates = coordinator.maintain().await?;
            latest.send_replace(candidates);
        }
    };
    let housekeeping = async {
        while changes.changed().await.is_ok() {
            let candidates = changes.borrow_and_update().clone();
            work(candidates).await?;
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
