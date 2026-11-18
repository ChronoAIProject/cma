//! Application readiness gates membership and every later ownership acquisition.
use super::*;
use crate::{health::HealthState, shutdown::Termination};

impl AgentManager {
    pub(crate) fn spawn_readiness(
        self: &Arc<Self>,
        health: HealthState,
        termination: Termination,
    ) -> Result<tokio::task::JoinHandle<()>, String> {
        let weak = Arc::downgrade(self);
        let stop = termination.clone();
        self.coordinator
            .set_admission_readiness(Arc::new(move || {
                !stop.requested()
                    && health.ready()
                    && weak
                        .upgrade()
                        .is_some_and(|manager| manager.serving.load(Ordering::Acquire))
            }))
            .map_err(|error| error.to_string())?;
        let weak = Arc::downgrade(self);
        Ok(tokio::spawn(async move {
            loop {
                let Some(manager) = weak.upgrade() else {
                    return;
                };
                if termination.requested() {
                    manager.stop_admission();
                    return;
                }
                if manager.coordinator.is_draining() {
                    return;
                }
                if manager.coordinator.is_starting() && manager.coordinator.activate().await.is_ok()
                {
                    tracing::info!("Agent Manager ownership activated after serving readiness");
                }
                drop(manager);
                tokio::select! {
                    biased;
                    _ = termination.cancelled() => {
                        if let Some(manager) = weak.upgrade() { manager.stop_admission(); }
                        return;
                    },
                    _ = tokio::time::sleep(Duration::from_millis(100)) => (),
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests;
